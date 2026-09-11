// 动图（showdown GIF 转 ANSI 帧）数据文件 anim.bin 的打包与运行时加载。
// 数据独立于主二进制：--anim-pack 打包生成，--animated 时按搜索路径懒加载，
// 文件缺失或损坏一律回退静态图，无 panic 路径。
//
// blob 格式（key 排序，供二分）：
//   n u32le
//   条目×n：klen u8 | key | rows u8 | cols u8
//           | n_frames u16le | delay_cs u16le | coff u32le | clen u32le
//   帧区：每条目的全部帧编码（codec::encode）串联后整体 zstd 一帧，coff 以帧区起点为 0

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::codec;
use crate::die;

/// 打包条目：帧编码串联 + 尺寸（帧间同宽高，由转换期联合 bbox 保证）
struct Packed {
    key: String,
    rows: u8,
    cols: u8,
    delay_cs: u16,
    n_frames: usize,
    frames: Vec<u8>,
}

/// 单帧：解析 + round-trip 语义断言（与 build.rs 静态闸门同口径），返回编码字节
fn encode_frame(key: &str, text: &str) -> Vec<u8> {
    let sprite = std::panic::catch_unwind(|| codec::parse_ansi(text))
        .unwrap_or_else(|p| die(&format!("{key}: 帧解析失败: {}", panic_msg(p))));
    let mut rendered = String::new();
    codec::decode_and_render(&codec::encode(&sprite), &mut rendered);
    let re = std::panic::catch_unwind(|| codec::parse_ansi(&rendered))
        .unwrap_or_else(|p| die(&format!("{key}: round-trip 再解析失败: {}", panic_msg(p))));
    if !codec::semantic_eq(&sprite, &re) {
        die(&format!("{key}: round-trip 语义不一致"));
    }
    codec::encode(&sprite)
}

fn panic_msg(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "未知 panic".into())
}

/// 帧目录布局：<dir>/<资产名>/{regular,shiny}/NNNN.ans + meta（delay=<cs> 行）
/// 产出写到 out 路径（维护命令 --anim-pack 的落点，通常为 anim.bin）
pub(crate) fn pack(dir: &Path, out: &Path) {
    let mut items: Vec<Packed> = Vec::new();
    let mut assets: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| die(&format!("读目录 {dir:?} 失败: {e}")))
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assets.sort();
    for asset in &assets {
        for variant in ["regular", "shiny"] {
            let vdir = dir.join(asset).join(variant);
            let key = format!("{variant}/{asset}");
            if !vdir.is_dir() {
                continue; // shiny 缺位与上游一致，允许单变体
            }
            let meta = std::fs::read_to_string(vdir.join("meta"))
                .unwrap_or_else(|e| die(&format!("{key}: 读 meta 失败: {e}")));
            let delay_cs: u16 = meta
                .lines()
                .find_map(|l| l.strip_prefix("delay="))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or_else(|| die(&format!("{key}: meta 缺 delay 行")));
            let mut files: Vec<_> = std::fs::read_dir(&vdir)
                .unwrap_or_else(|e| die(&format!("读目录 {vdir:?} 失败: {e}")))
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "ans"))
                .collect();
            files.sort();
            if files.is_empty() {
                die(&format!("{key}: 无帧文件"));
            }
            let mut frames = Vec::new();
            let (mut rows, mut cols) = (None, None);
            for f in &files {
                let text = std::fs::read_to_string(f)
                    .unwrap_or_else(|e| die(&format!("{key}: 读 {:?} 失败: {e}", f.file_name())));
                let encoded = encode_frame(&key, &text);
                let (r, c) = (encoded[0], encoded[1]);
                match (rows, cols) {
                    (Some(r0), Some(c0)) if (r0, c0) != (r, c) => die(&format!(
                        "{key}: 帧尺寸不一致 {:?}: {c0}x{r0} vs {c}x{r}",
                        f.file_name()
                    )),
                    (None, _) => (rows, cols) = (Some(r), Some(c)),
                    _ => {}
                }
                frames.extend_from_slice(&encoded);
            }
            let frame_count = files.len();
            if frame_count > u16::MAX as usize {
                die(&format!(
                    "{key}: 帧数超上限（{frame_count} > {}）",
                    u16::MAX
                ));
            }
            items.push(Packed {
                key,
                rows: rows.unwrap(),
                cols: cols.unwrap(),
                delay_cs,
                n_frames: frame_count,
                frames,
            });
        }
    }
    if items.is_empty() {
        die(&format!("{dir:?} 下没有可打包的动画条目"));
    }
    items.sort_by(|a, b| a.key.cmp(&b.key));

    // 组 blob：索引区预留回填（同 build.rs 手法），帧区偏移以帧区起点为 0
    let mut blob = Vec::new();
    blob.extend_from_slice(&(items.len() as u32).to_le_bytes());
    let mut index_size = 4usize;
    for it in &items {
        index_size += 1 + it.key.len() + 14;
    }
    blob.resize(index_size, 0);
    let mut slot = 4usize;
    for it in &items {
        let frame = ruzstd::encoding::compress_to_vec(
            it.frames.as_slice(),
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        // 压缩后立即解压回比：编码字节必须无损
        let mut dec = ruzstd::decoding::StreamingDecoder::new(frame.as_slice())
            .unwrap_or_else(|e| die(&format!("{}: 解码器初始化失败: {e}", it.key)));
        let mut got = Vec::with_capacity(it.frames.len());
        dec.read_to_end(&mut got)
            .unwrap_or_else(|e| die(&format!("{}: 解码失败: {e}", it.key)));
        assert!(got == it.frames, "{}: 压缩帧解压后不一致", it.key);

        let coff = (blob.len() - index_size) as u32;
        let clen = frame.len() as u32;
        blob.extend_from_slice(&frame);
        let n = it.key.len();
        blob[slot] = n as u8;
        blob[slot + 1..slot + 1 + n].copy_from_slice(it.key.as_bytes());
        let p = slot + 1 + n;
        blob[p] = it.rows;
        blob[p + 1] = it.cols;
        blob[p + 2..p + 4].copy_from_slice(&it.delay_cs.to_le_bytes());
        blob[p + 4..p + 6].copy_from_slice(&(it.n_frames as u16).to_le_bytes());
        blob[p + 6..p + 10].copy_from_slice(&coff.to_le_bytes());
        blob[p + 10..p + 14].copy_from_slice(&clen.to_le_bytes());
        slot = p + 14;
    }

    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|e| die(&format!("建目录 {parent:?} 失败: {e}")));
    }
    let size = std::fs::write(out, &blob).map(|_| blob.len());
    match size {
        Ok(n) => eprintln!(
            "anim-pack: {} 条动画，写入 {:?}（{:.1}MB）",
            items.len(),
            out,
            n as f64 / 1e6
        ),
        Err(e) => die(&format!("写 {out:?} 失败: {e}")),
    }
}

/// 运行时动画条目
pub(crate) struct Animation {
    pub(crate) frames: Vec<String>,
    pub(crate) delay: Duration,
}

/// 已加载的 anim.bin：持有数据与解析好的索引
pub(crate) struct AnimData {
    data: Vec<u8>,
    index: Vec<(String, u8, u8, u16, usize, usize, usize)>, // key, rows, cols, delay_cs, n_frames, coff, clen
}

impl AnimData {
    /// 搜索顺序：$POKEFETCH_ANIM > $XDG_DATA_HOME/pokefetch/anim.bin（默认 ~/.local/share）
    pub(crate) fn load() -> Option<AnimData> {
        let path = std::env::var("POKEFETCH_ANIM")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                let base = std::env::var("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .ok()
                    .or_else(|| {
                        std::env::var("HOME")
                            .ok()
                            .map(|h| PathBuf::from(h).join(".local/share"))
                    })?;
                Some(base.join("pokefetch").join("anim.bin"))
            })?;
        let Ok(bytes) = std::fs::read(&path) else {
            return None;
        };
        let parsed = Self::parse(&bytes);
        if parsed.is_none() {
            eprintln!("pokefetch: 动画数据 {:?} 无法解析，回退静态图", path);
        }
        parsed
    }

    fn parse(data: &[u8]) -> Option<AnimData> {
        if data.len() < 4 {
            return None;
        }
        let n = u32::from_le_bytes(data[..4].try_into().ok()?) as usize;
        let mut index = Vec::with_capacity(n);
        let mut pos = 4usize;
        for _ in 0..n {
            let klen = *data.get(pos)? as usize;
            let key = std::str::from_utf8(data.get(pos + 1..pos + 1 + klen)?)
                .ok()?
                .to_string();
            let p = pos + 1 + klen;
            if p + 14 > data.len() {
                return None;
            }
            let rows = data[p];
            let cols = data[p + 1];
            let delay_cs = u16::from_le_bytes(data[p + 2..p + 4].try_into().ok()?);
            let n_frames = u16::from_le_bytes(data[p + 4..p + 6].try_into().ok()?) as usize;
            let coff = u32::from_le_bytes(data[p + 6..p + 10].try_into().ok()?) as usize;
            let clen = u32::from_le_bytes(data[p + 10..p + 14].try_into().ok()?) as usize;
            index.push((key, rows, cols, delay_cs, n_frames, coff, clen));
            pos = p + 14;
        }
        let frames_start = pos;
        for (_, _, _, _, _, coff, _) in &mut index {
            *coff += frames_start;
        }
        Some(AnimData {
            data: data.to_vec(),
            index,
        })
    }

    /// 命中 key（形如 "regular/pikachu"）则解压并逐帧渲染为 ANSI 文本
    pub(crate) fn lookup(&self, key: &str) -> Option<Animation> {
        let i = self
            .index
            .binary_search_by(|(k, ..)| k.as_str().cmp(key))
            .ok()?;
        let (_, _, _, delay_cs, n_frames, coff, clen) = self.index[i];
        let mut dec =
            ruzstd::decoding::StreamingDecoder::new(&self.data[coff..coff + clen]).ok()?;
        let mut packed = Vec::new();
        dec.read_to_end(&mut packed).ok()?;
        let mut frames = Vec::with_capacity(n_frames);
        let mut pos = 0usize;
        for _ in 0..n_frames {
            let rest = packed.get(pos..)?;
            let size = codec::encoded_size(rest);
            let mut ansi = String::new();
            codec::decode_and_render(rest.get(..size)?, &mut ansi);
            frames.push(ansi);
            pos += size;
        }
        Some(Animation {
            frames,
            delay: Duration::from_millis(u64::from(delay_cs) * 10),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 按运行时格式手工构造单条目 blob（key/rows/cols/delay/帧编码串联）
    fn build_blob(key: &str, rows: u8, cols: u8, delay_cs: u16, frames: &[u8]) -> Vec<u8> {
        let comp =
            ruzstd::encoding::compress_to_vec(frames, ruzstd::encoding::CompressionLevel::Fastest);
        let mut blob = Vec::new();
        blob.extend_from_slice(&1u32.to_le_bytes());
        let index_size = 4 + 1 + key.len() + 14;
        blob.resize(index_size, 0);
        blob.extend_from_slice(&comp);
        blob[4] = key.len() as u8;
        blob[5..5 + key.len()].copy_from_slice(key.as_bytes());
        let p = 5 + key.len();
        blob[p] = rows;
        blob[p + 1] = cols;
        blob[p + 2..p + 4].copy_from_slice(&delay_cs.to_le_bytes());
        // 帧数由 encoded_size 切分反推：测试里直接写死
        let mut n = 0usize;
        let mut pos = 0usize;
        while pos < frames.len() {
            pos += codec::encoded_size(&frames[pos..]);
            n += 1;
        }
        blob[p + 4..p + 6].copy_from_slice(&(n as u16).to_le_bytes());
        blob[p + 6..p + 10].copy_from_slice(&0u32.to_le_bytes());
        blob[p + 10..p + 14].copy_from_slice(&(comp.len() as u32).to_le_bytes());
        blob
    }

    #[test]
    fn parse_and_lookup_roundtrip() {
        let f1 = codec::encode(&codec::parse_ansi("█\n"));
        let f2 = codec::encode(&codec::parse_ansi("\x1b[38;2;255;0;0m█\x1b[0m\n"));
        let mut frames = f1;
        frames.extend_from_slice(&f2);
        let blob = build_blob("regular/pikachu", 1, 1, 4, &frames);
        let data = AnimData::parse(&blob).unwrap();
        let anim = data.lookup("regular/pikachu").unwrap();
        assert_eq!(anim.frames.len(), 2);
        assert!(anim.frames[0].contains('█'));
        assert!(anim.frames[1].contains("38;2;255;0;0"));
        assert_eq!(anim.delay, Duration::from_millis(40));
        assert!(data.lookup("regular/不存在").is_none());
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(AnimData::parse(b"").is_none());
        assert!(AnimData::parse(&[9, 0, 0, 0, 255]).is_none()); // n 超出数据
    }
}
