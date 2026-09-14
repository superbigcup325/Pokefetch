// 动图（showdown GIF 转 ANSI 帧）数据文件 anim.bin 的打包与运行时加载。
// 数据独立于主二进制：--anim-pack 打包生成，--animated 时按搜索路径懒加载，
// 文件缺失或损坏一律回退静态图，无 panic 路径。
//
// v2 格式（当前，"PKFA" + 版本号；key 排序供二分）：
//   magic "PKFA" | ver u8 = 2
//   n u32le
//   条目×n：klen u8 | key | rows u8 | cols u8 | delay_cs u16le | n_frames u16le
//           | wide u8 | pal_n u16le | pal pal_n×3B | coff u32le | clen u32le
//   帧区：每条目 = 首帧全量格子块 + 差分帧×(n_frames-1)，整体 zstd 一帧，
//         coff 以帧区起点为 0
//   格子 = wide ? (fg u16le, bg u16le, glyph u8) : (fg u8, bg u8, glyph u8)，
//         wide = 条目级调色板超 255 色时索引升位
//   调色板为条目级共享（v1 逐帧自带是体积大头之一）
//   差分帧 = (skip u16le, count u16le, count×格子)*，累计覆盖 rows×cols 格，
//            末段 count=0 收尾（全未变帧即单段 (格数, 0)）
//
// v1 遗留格式（无 magic，只读兼容）：
//   n u32le
//   条目×n：klen u8 | key | rows u8 | cols u8
//           | n_frames u16le | delay_cs u16le | coff u32le | clen u32le
//   帧区：每条目的全部帧 codec::encode 编码串联（调色板逐帧自带）后整体 zstd 一帧

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::codec::{self, Color};
use crate::die;

const MAGIC: &[u8; 4] = b"PKFA";
const VERSION: u8 = 2;

/// 打包条目：调色板并集 + 首帧全量/差分帧体（未压缩，帧间同宽高由转换期联合 bbox 保证）
struct EntryFrames {
    key: String,
    rows: u8,
    cols: u8,
    delay_cs: u16,
    n_frames: usize,
    wide: bool,
    pal: Vec<Color>,
    body: Vec<u8>,
}

/// 解析帧文本 + round-trip 语义断言（与 build.rs 静态闸门同口径）
fn parse_checked(key: &str, text: &str) -> codec::Sprite {
    let sprite = std::panic::catch_unwind(|| codec::parse_ansi(text))
        .unwrap_or_else(|p| die(&format!("{key}: 帧解析失败: {}", panic_msg(p))));
    let mut rendered = String::new();
    codec::decode_and_render(&codec::encode(&sprite), &mut rendered);
    let re = std::panic::catch_unwind(|| codec::parse_ansi(&rendered))
        .unwrap_or_else(|p| die(&format!("{key}: round-trip 再解析失败: {}", panic_msg(p))));
    if !codec::semantic_eq(&sprite, &re) {
        die(&format!("{key}: round-trip 语义不一致"));
    }
    sprite
}

fn panic_msg(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "未知 panic".into())
}

/// 差分帧编码：段 = (skip u16le, count u16le, count×格)，末段 count=0 收尾。
/// 画布至多 255×255=65025 格，skip/count 单字段必不溢出
fn delta_encode(prev: &[u8], cur: &[u8], step: usize, out: &mut Vec<u8>) {
    debug_assert_eq!(prev.len(), cur.len());
    let cells = cur.len() / step;
    let mut covered = 0usize;
    let mut i = 0usize;
    while i < cells {
        if prev[i * step..i * step + step] == cur[i * step..i * step + step] {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < cells && prev[j * step..j * step + step] != cur[j * step..j * step + step] {
            j += 1;
        }
        out.extend_from_slice(&((i - covered) as u16).to_le_bytes());
        out.extend_from_slice(&((j - i) as u16).to_le_bytes());
        out.extend_from_slice(&cur[i * step..j * step]);
        covered = j;
        i = j;
    }
    // 末段变更已覆盖满时无收尾段；全未变帧即单段 (格数, 0)
    if covered < cells {
        out.extend_from_slice(&((cells - covered) as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
    }
}

/// 差分帧应用：grid 就地推进为当前帧，返回消费的字节数；残缺返回 None
fn delta_decode(grid: &mut [u8], delta: &[u8], step: usize) -> Option<usize> {
    let cells = grid.len() / step;
    let mut src = 0usize;
    let mut covered = 0usize;
    while covered < cells {
        let skip = u16_at(delta, src)? as usize;
        let count = u16_at(delta, src + 2)? as usize;
        src += 4;
        covered += skip;
        if covered + count > cells {
            return None;
        }
        let bytes = count * step;
        grid[covered * step..(covered + count) * step]
            .copy_from_slice(delta.get(src..src + bytes)?);
        covered += count;
        src += bytes;
    }
    Some(src)
}

fn u16_at(data: &[u8], pos: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(pos..pos + 2)?.try_into().ok()?))
}

/// 单条目编码：帧间调色板并集 → u16 索引格子按位宽收敛 → 首帧全量 + 差分帧
fn encode_entry(key: &str, delay_cs: u16, texts: &[String]) -> EntryFrames {
    if texts.is_empty() {
        die(&format!("{key}: 无帧文件"));
    }
    if texts.len() > u16::MAX as usize {
        die(&format!(
            "{key}: 帧数超上限（{} > {}）",
            texts.len(),
            u16::MAX
        ));
    }
    let sprites: Vec<codec::Sprite> = texts.iter().map(|t| parse_checked(key, t)).collect();
    let (rows, cols) = (sprites[0].rows, sprites[0].cols);
    for (i, s) in sprites.iter().enumerate().skip(1) {
        if (s.rows, s.cols) != (rows, cols) {
            die(&format!(
                "{key}: 第 {i} 帧尺寸不一致 {cols}x{rows} vs {}x{}",
                s.cols, s.rows
            ));
        }
    }
    if rows > u8::MAX as usize || cols > u8::MAX as usize {
        die(&format!("{key}: 帧尺寸超 u8（{cols}x{rows}）"));
    }

    // 调色板并集；格子先按 u16 索引展开（此时位宽未定），收敛时按并集大小落盘
    let mut pal: Vec<Color> = Vec::new();
    let mut rev: HashMap<Color, u16> = HashMap::new();
    let grids: Vec<Vec<u8>> = sprites
        .iter()
        .map(|s| {
            let mut grid = Vec::with_capacity(s.cells.len() * 5);
            for &(f, b, g) in &s.cells {
                let mut idx = |v: u16| -> u16 {
                    if v == 0 {
                        0
                    } else {
                        let c = s.pal[v as usize - 1];
                        *rev.entry(c).or_insert_with(|| {
                            pal.push(c);
                            pal.len() as u16
                        })
                    }
                };
                grid.extend_from_slice(&idx(f).to_le_bytes());
                grid.extend_from_slice(&idx(b).to_le_bytes());
                grid.push(g);
            }
            grid
        })
        .collect();
    let wide = pal.len() > 255;
    let step = if wide { 5 } else { 3 };
    let grids: Vec<Vec<u8>> = if wide {
        grids
    } else {
        // u16 → u8 索引收敛（并集 ≤255 保证高位为 0）：取 fg/bg 低字节 + glyph
        grids
            .iter()
            .map(|g| {
                g.as_chunks::<5>()
                    .0
                    .iter()
                    .flat_map(|c| [c[0], c[2], c[4]])
                    .collect()
            })
            .collect()
    };
    let mut body = Vec::new();
    let mut prev: &[u8] = &[];
    for (i, g) in grids.iter().enumerate() {
        if i == 0 {
            body.extend_from_slice(g);
        } else {
            delta_encode(prev, g, step, &mut body);
        }
        prev = g;
    }
    EntryFrames {
        key: key.to_string(),
        rows: rows as u8,
        cols: cols as u8,
        delay_cs,
        n_frames: grids.len(),
        wide,
        pal,
        body,
    }
}

/// 组 v2 blob：索引区预留回填（同 build.rs 手法），帧区逐条目 zstd + 解压回比断言
fn assemble(items: &[EntryFrames]) -> Vec<u8> {
    let mut blob = Vec::new();
    blob.extend_from_slice(MAGIC);
    blob.push(VERSION);
    blob.extend_from_slice(&(items.len() as u32).to_le_bytes());
    let mut index_size = 9usize;
    for it in items {
        index_size += 1 + it.key.len() + 17 + it.pal.len() * 3;
    }
    blob.resize(index_size, 0);
    let mut slot = 9usize;
    for it in items {
        let frame = ruzstd::encoding::compress_to_vec(
            it.body.as_slice(),
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        // 压缩后立即解压回比：编码字节必须无损
        let mut dec = ruzstd::decoding::StreamingDecoder::new(frame.as_slice())
            .unwrap_or_else(|e| die(&format!("{}: 解码器初始化失败: {e}", it.key)));
        let mut got = Vec::with_capacity(it.body.len());
        dec.read_to_end(&mut got)
            .unwrap_or_else(|e| die(&format!("{}: 解码失败: {e}", it.key)));
        assert!(got == it.body, "{}: 压缩帧解压后不一致", it.key);

        let coff = (blob.len() - index_size) as u32;
        let clen = frame.len() as u32;
        blob.extend_from_slice(&frame);
        blob[slot] = it.key.len() as u8;
        blob[slot + 1..slot + 1 + it.key.len()].copy_from_slice(it.key.as_bytes());
        let p = slot + 1 + it.key.len();
        blob[p] = it.rows;
        blob[p + 1] = it.cols;
        blob[p + 2..p + 4].copy_from_slice(&it.delay_cs.to_le_bytes());
        blob[p + 4..p + 6].copy_from_slice(&(it.n_frames as u16).to_le_bytes());
        blob[p + 6] = it.wide as u8;
        blob[p + 7..p + 9].copy_from_slice(&(it.pal.len() as u16).to_le_bytes());
        let pb = p + 9;
        for (i, (r, g, b)) in it.pal.iter().enumerate() {
            blob[pb + i * 3..pb + i * 3 + 3].copy_from_slice(&[*r, *g, *b]);
        }
        let cb = pb + it.pal.len() * 3;
        blob[cb..cb + 4].copy_from_slice(&coff.to_le_bytes());
        blob[cb + 4..cb + 8].copy_from_slice(&clen.to_le_bytes());
        slot = cb + 8;
    }
    blob
}

/// 帧目录布局：<dir>/<资产名>/{regular,shiny}/NNNN.ans + meta（delay=<cs> 行）
/// 产出写到 out 路径（维护命令 --anim-pack 的落点，通常为 anim.bin），恒为 v2 格式
pub(crate) fn pack(dir: &Path, out: &Path) {
    let mut items: Vec<EntryFrames> = Vec::new();
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
            let texts: Vec<String> = files
                .iter()
                .map(|f| {
                    std::fs::read_to_string(f).unwrap_or_else(|e| {
                        die(&format!("{key}: 读 {:?} 失败: {e}", f.file_name()))
                    })
                })
                .collect();
            items.push(encode_entry(&key, delay_cs, &texts));
        }
    }
    if items.is_empty() {
        die(&format!("{dir:?} 下没有可打包的动画条目"));
    }
    items.sort_by(|a, b| a.key.cmp(&b.key));

    let blob = assemble(&items);
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
    v2: bool,
    entries: Vec<Entry>,
}

struct Entry {
    key: String,
    rows: u8,
    cols: u8,
    delay_cs: u16,
    n_frames: usize,
    coff: usize,
    clen: usize,
    wide: bool,
    pal: Vec<Color>, // v1 条目为空（调色板逐帧自带）
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

    /// 按 magic 分流：v2 现行格式 / v1 遗留只读兼容
    fn parse(data: &[u8]) -> Option<AnimData> {
        if data.len() >= 5 && data[..4] == *MAGIC {
            if data[4] != VERSION {
                return None;
            }
            Some(AnimData {
                data: data.to_vec(),
                v2: true,
                entries: parse_v2(data)?,
            })
        } else {
            Some(AnimData {
                data: data.to_vec(),
                v2: false,
                entries: parse_v1(data)?,
            })
        }
    }

    /// 命中 key（形如 "regular/pikachu"）则解压并逐帧渲染为 ANSI 文本
    pub(crate) fn lookup(&self, key: &str) -> Option<Animation> {
        let i = self
            .entries
            .binary_search_by(|e| e.key.as_str().cmp(key))
            .ok()?;
        let e = &self.entries[i];
        // pack 不会产出 0 帧条目，坏文件按缺失处理
        if e.n_frames == 0 {
            return None;
        }
        let mut dec =
            ruzstd::decoding::StreamingDecoder::new(&self.data[e.coff..e.coff + e.clen]).ok()?;
        let mut packed = Vec::new();
        dec.read_to_end(&mut packed).ok()?;
        let mut frames = Vec::with_capacity(e.n_frames);
        if self.v2 {
            let step = if e.wide { 5 } else { 3 };
            let total = codec::cells_len(e.rows as usize, e.cols as usize, e.wide);
            let mut grid = packed.get(..total)?.to_vec();
            let render = |grid: &[u8], frames: &mut Vec<String>| {
                let mut s = String::new();
                codec::render_cells(
                    &e.pal,
                    e.rows as usize,
                    e.cols as usize,
                    e.wide,
                    grid,
                    &mut s,
                );
                frames.push(s);
            };
            render(&grid, &mut frames);
            let mut pos = total;
            for _ in 1..e.n_frames {
                let rest = packed.get(pos..)?;
                if rest.len() < 4 {
                    return None; // 一段 (skip, count) 至少 4B
                }
                let used = delta_decode(&mut grid, rest, step)?;
                render(&grid, &mut frames);
                pos += used;
            }
        } else {
            // v1：帧编码自带调色板，encoded_size 切帧
            let mut pos = 0usize;
            for _ in 0..e.n_frames {
                let rest = packed.get(pos..)?;
                // 帧头至少 5B（rows|cols|wide|n_pal），坏尾巴拒绝而非越界 panic
                if rest.len() < 5 {
                    return None;
                }
                let size = codec::encoded_size(rest);
                let mut ansi = String::new();
                codec::decode_and_render(rest.get(..size)?, &mut ansi);
                frames.push(ansi);
                pos += size;
            }
        }
        Some(Animation {
            frames,
            delay: Duration::from_millis(u64::from(e.delay_cs) * 10),
        })
    }
}

/// v1 遗留格式解析（只读）
fn parse_v1(data: &[u8]) -> Option<Vec<Entry>> {
    if data.len() < 4 {
        return None;
    }
    let n = u32::from_le_bytes(data[..4].try_into().ok()?) as usize;
    // 每条目至少 1+14B：先按数据量卡条目上限，防坏 n 把 with_capacity 撑爆
    if n > (data.len() - 4) / 15 {
        return None;
    }
    let mut entries = Vec::with_capacity(n);
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
        entries.push(Entry {
            key,
            rows: data[p],
            cols: data[p + 1],
            delay_cs: u16::from_le_bytes(data[p + 2..p + 4].try_into().ok()?),
            n_frames: u16::from_le_bytes(data[p + 4..p + 6].try_into().ok()?) as usize,
            coff: u32::from_le_bytes(data[p + 6..p + 10].try_into().ok()?) as usize,
            clen: u32::from_le_bytes(data[p + 10..p + 14].try_into().ok()?) as usize,
            wide: false,
            pal: Vec::new(),
        });
        pos = p + 14;
    }
    finish_entries(entries, data, pos)
}

/// v2 现行格式解析
fn parse_v2(data: &[u8]) -> Option<Vec<Entry>> {
    if data.len() < 9 {
        return None;
    }
    let n = u32::from_le_bytes(data[5..9].try_into().ok()?) as usize;
    // 每条目至少 1+17B（pal_n=0），同 v1 卡上限
    if n > (data.len() - 9) / 18 {
        return None;
    }
    let mut entries = Vec::with_capacity(n);
    let mut pos = 9usize;
    for _ in 0..n {
        let klen = *data.get(pos)? as usize;
        let key = std::str::from_utf8(data.get(pos + 1..pos + 1 + klen)?)
            .ok()?
            .to_string();
        let p = pos + 1 + klen;
        if p + 9 > data.len() {
            return None;
        }
        let pal_n = u16::from_le_bytes(data[p + 7..p + 9].try_into().ok()?) as usize;
        let pb = p + 9;
        let pal: Vec<Color> = data
            .get(pb..pb + pal_n * 3)?
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| (c[0], c[1], c[2]))
            .collect();
        if pal.len() != pal_n {
            return None; // 调色板残尾
        }
        let cb = pb + pal_n * 3;
        if cb + 8 > data.len() {
            return None;
        }
        entries.push(Entry {
            key,
            rows: data[p],
            cols: data[p + 1],
            delay_cs: u16::from_le_bytes(data[p + 2..p + 4].try_into().ok()?),
            n_frames: u16::from_le_bytes(data[p + 4..p + 6].try_into().ok()?) as usize,
            wide: data[p + 6] != 0,
            coff: u32::from_le_bytes(data[cb..cb + 4].try_into().ok()?) as usize,
            clen: u32::from_le_bytes(data[cb + 4..cb + 8].try_into().ok()?) as usize,
            pal,
        });
        pos = cb + 8;
    }
    finish_entries(entries, data, pos)
}

/// 帧区起点回填 + 偏移越界整文件拒绝（lookup 侧切片即恒在界内）
fn finish_entries(mut entries: Vec<Entry>, data: &[u8], frames_start: usize) -> Option<Vec<Entry>> {
    for e in &mut entries {
        if frames_start + e.coff + e.clen > data.len() {
            return None;
        }
        e.coff += frames_start;
    }
    Some(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 按 v1 遗留格式构造多条目 blob（key 须有序；coff 以帧区起点为 0，同 pack 语义）。
    /// 帧数显式给定，便于伪造坏索引。条目 = (key, rows, cols, delay_cs, n_frames, 帧编码串联)
    type TestItem<'a> = (&'a str, u8, u8, u16, usize, Vec<u8>);

    fn build_multi_blob(items: &[TestItem<'_>]) -> Vec<u8> {
        let comp: Vec<Vec<u8>> = items
            .iter()
            .map(|(.., frames)| {
                ruzstd::encoding::compress_to_vec(
                    frames.as_slice(),
                    ruzstd::encoding::CompressionLevel::Fastest,
                )
            })
            .collect();
        let mut blob = Vec::new();
        blob.extend_from_slice(&(items.len() as u32).to_le_bytes());
        let mut index_size = 4usize;
        for (k, ..) in items {
            index_size += 1 + k.len() + 14;
        }
        blob.resize(index_size, 0);
        let mut slot = 4usize;
        for ((k, r, c, d, n, _), comp) in items.iter().zip(comp) {
            let coff = (blob.len() - index_size) as u32;
            let clen = comp.len() as u32;
            blob.extend_from_slice(&comp);
            blob[slot] = k.len() as u8;
            blob[slot + 1..slot + 1 + k.len()].copy_from_slice(k.as_bytes());
            let p = slot + 1 + k.len();
            blob[p] = *r;
            blob[p + 1] = *c;
            blob[p + 2..p + 4].copy_from_slice(&d.to_le_bytes());
            blob[p + 4..p + 6].copy_from_slice(&(*n as u16).to_le_bytes());
            blob[p + 6..p + 10].copy_from_slice(&coff.to_le_bytes());
            blob[p + 10..p + 14].copy_from_slice(&clen.to_le_bytes());
            slot = p + 14;
        }
        blob
    }

    /// v1 单条目 blob，帧区直接给定（可注入任意字节流，不必是合法帧编码）
    fn blob_with_region(region: &[u8], n_frames: u16) -> Vec<u8> {
        let key = "regular/a";
        let mut blob = Vec::new();
        blob.extend_from_slice(&1u32.to_le_bytes());
        let index_size = 4 + 1 + key.len() + 14;
        blob.resize(index_size, 0);
        blob.extend_from_slice(region);
        blob[4] = key.len() as u8;
        blob[5..5 + key.len()].copy_from_slice(key.as_bytes());
        let p = 5 + key.len();
        blob[p] = 1;
        blob[p + 1] = 1;
        blob[p + 2..p + 4].copy_from_slice(&3u16.to_le_bytes());
        blob[p + 4..p + 6].copy_from_slice(&n_frames.to_le_bytes());
        blob[p + 6..p + 10].copy_from_slice(&0u32.to_le_bytes());
        blob[p + 10..p + 14].copy_from_slice(&(region.len() as u32).to_le_bytes());
        blob
    }

    /// 串联帧编码里的实际帧数（encoded_size 切分）
    fn count_frames(frames: &[u8]) -> usize {
        let mut n = 0usize;
        let mut pos = 0usize;
        while pos < frames.len() {
            pos += codec::encoded_size(&frames[pos..]);
            n += 1;
        }
        n
    }

    fn one_frame(text: &str) -> Vec<u8> {
        codec::encode(&codec::parse_ansi(text))
    }

    /// LCG 伪随机字节（测试确定性）
    struct Lcg(u32);
    impl Lcg {
        fn next(&mut self) -> u8 {
            self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
            (self.0 >> 16) as u8
        }
    }

    #[test]
    fn parse_and_lookup_roundtrip() {
        let mut frames = one_frame("█\n");
        frames.extend_from_slice(&one_frame("\x1b[38;2;255;0;0m█\x1b[0m\n"));
        let n = count_frames(&frames);
        let blob = build_multi_blob(&[("regular/pikachu", 1, 1, 4, n, frames)]);
        let data = AnimData::parse(&blob).unwrap();
        assert!(!data.v2); // 无 magic 走 v1 兼容路径
        let anim = data.lookup("regular/pikachu").unwrap();
        assert_eq!(anim.frames.len(), 2);
        assert!(anim.frames[0].contains('█'));
        assert!(anim.frames[1].contains("38;2;255;0;0"));
        assert_eq!(anim.delay, Duration::from_millis(40));
        assert!(data.lookup("regular/不存在").is_none());
    }

    #[test]
    fn multi_entry_lookup_and_delay() {
        let f = one_frame("█\n");
        let mut two = f.clone();
        two.extend_from_slice(&one_frame("\x1b[48;2;0;0;255m▀\x1b[0m\n"));
        let blob = build_multi_blob(&[
            ("regular/aaa", 1, 1, 0, count_frames(&f), f),
            ("shiny/bbb", 1, 1, 65535, count_frames(&two), two),
        ]);
        let data = AnimData::parse(&blob).unwrap();
        let a = data.lookup("regular/aaa").unwrap();
        assert_eq!(a.frames.len(), 1);
        assert_eq!(a.delay, Duration::ZERO);
        let b = data.lookup("shiny/bbb").unwrap();
        assert_eq!(b.frames.len(), 2);
        assert_eq!(b.delay, Duration::from_millis(655_350));
        // 二分命中要求 key 有序：互换变体名不得误命中
        assert!(data.lookup("regular/bbb").is_none());
        assert!(data.lookup("shiny/aaa").is_none());
    }

    #[test]
    fn wide_frame_roundtrip() {
        // >255 色触发 u16 索引路径，经 blob 往返渲染语义不丢
        let mut text = String::new();
        for i in 0..304u16 {
            if i > 0 && i % 16 == 0 {
                text.push('\n');
            }
            text.push_str(&format!("\x1b[38;2;{};{};0m█\x1b[0m", i & 0xff, i >> 8));
        }
        let f = codec::encode(&codec::parse_ansi(&text));
        let blob = build_multi_blob(&[("regular/wide", 19, 16, 4, 1, f)]);
        let data = AnimData::parse(&blob).unwrap();
        let anim = data.lookup("regular/wide").unwrap();
        let s1 = codec::parse_ansi(&text);
        let s2 = codec::parse_ansi(&anim.frames[0]);
        assert!(codec::semantic_eq(&s1, &s2));
    }

    #[test]
    fn parse_accepts_empty_index() {
        let data = AnimData::parse(&0u32.to_le_bytes()).unwrap();
        assert!(data.lookup("regular/pikachu").is_none());
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(AnimData::parse(b"").is_none());
        assert!(AnimData::parse(&[9, 0, 0, 0, 255]).is_none()); // n 超出数据
        // n=0xFFFFFFFF 配几字节残料：拒绝而非按 n 预留容量
        assert!(AnimData::parse(&[0xff, 0xff, 0xff, 0xff, 1, 2, 3]).is_none());
    }

    #[test]
    fn parse_rejects_truncated_file() {
        let blob = build_multi_blob(&[("regular/aaaaaaaa", 1, 1, 3, 1, one_frame("█\n"))]);
        assert!(AnimData::parse(&blob[..blob.len() / 2]).is_none()); // 索引区腰斩
        assert!(AnimData::parse(&blob[..blob.len() - 1]).is_none()); // 帧区截断同拒
    }

    #[test]
    fn parse_rejects_frame_range_out_of_bounds() {
        let p = 4 + 1 + "regular/a".len(); // 首条目 rows 字节位置
        let mut blob = build_multi_blob(&[("regular/a", 1, 1, 3, 1, one_frame("█\n"))]);
        blob[p + 10..p + 14].copy_from_slice(&u32::MAX.to_le_bytes()); // clen 越界
        assert!(AnimData::parse(&blob).is_none());
        let mut blob = build_multi_blob(&[("regular/a", 1, 1, 3, 1, one_frame("█\n"))]);
        blob[p + 6..p + 10].copy_from_slice(&u32::MAX.to_le_bytes()); // coff 越界
        assert!(AnimData::parse(&blob).is_none());
    }

    #[test]
    fn parse_rejects_bad_utf8_key() {
        let mut blob = Vec::new();
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&[1, 0xff]); // klen=1 + 非 UTF-8 key
        blob.extend_from_slice(&[0; 14]);
        assert!(AnimData::parse(&blob).is_none());
    }

    #[test]
    fn lookup_rejects_degenerate_frames() {
        // n_frames=0：pack 不可产出，坏文件按缺失处理
        let blob = build_multi_blob(&[("regular/a", 1, 1, 3, 0, one_frame("█\n"))]);
        let data = AnimData::parse(&blob).unwrap();
        assert!(data.lookup("regular/a").is_none());

        // n_frames 虚高：帧流提前耗尽（残尾不足 5B 帧头）→ None 而非 panic
        let blob = build_multi_blob(&[("regular/a", 1, 1, 3, 3, one_frame("█\n"))]);
        let data = AnimData::parse(&blob).unwrap();
        assert!(data.lookup("regular/a").is_none());

        // 帧流解压后不足一个帧头
        let junk = ruzstd::encoding::compress_to_vec(
            &[1u8, 2, 3, 4][..],
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        let data = AnimData::parse(&blob_with_region(&junk, 1)).unwrap();
        assert!(data.lookup("regular/a").is_none());
    }

    #[test]
    fn delta_roundtrip() {
        // 伪随机帧序列（帧间 ~25% 格变化）：编码→应用必须还原，消费数=编码长度
        for step in [3usize, 5] {
            let mut rng = Lcg(0x1234_5678);
            let mut grids: Vec<Vec<u8>> = vec![(0..200 * step).map(|_| rng.next()).collect()];
            for _ in 0..4 {
                let mut g = grids.last().unwrap().clone();
                for b in g.iter_mut() {
                    if rng.next() < 64 {
                        *b = rng.next();
                    }
                }
                grids.push(g);
            }
            for pair in grids.windows(2) {
                let mut d = Vec::new();
                delta_encode(&pair[0], &pair[1], step, &mut d);
                let mut buf = pair[0].clone();
                let used = delta_decode(&mut buf, &d, step).unwrap();
                assert_eq!(buf, pair[1]);
                assert_eq!(used, d.len());
            }
            // 全未变帧：单段 (格数, 0)
            let mut d = Vec::new();
            delta_encode(&grids[0], &grids[0], step, &mut d);
            assert_eq!(d.len(), 4);
            let mut buf = grids[0].clone();
            assert_eq!(delta_decode(&mut buf, &d, step), Some(4));
            assert_eq!(buf, grids[0]);
        }
    }

    #[test]
    fn delta_rejects_truncated() {
        let g = vec![1u8; 9];
        let mut d = Vec::new();
        delta_encode(&[0u8; 9], &g, 3, &mut d);
        assert!(delta_decode(&mut [0u8; 9], &d[..d.len() - 2], 3).is_none());
        // skip+count 覆盖越界
        let mut bad = d.clone();
        bad[2..4].copy_from_slice(&999u16.to_le_bytes());
        assert!(delta_decode(&mut [0u8; 9], &bad, 3).is_none());
    }

    #[test]
    fn v2_pack_parse_roundtrip() {
        let texts = vec![
            "\x1b[38;2;255;0;0m█\x1b[0m\n".to_string(),
            "\x1b[48;2;0;0;255m█\x1b[0m\n".to_string(),
            "█\n".to_string(),
        ];
        let item = encode_entry("regular/rt", 4, &texts);
        assert!(!item.wide);
        let other = encode_entry("shiny/zz", 3, &["▀\n".to_string()]);
        let blob = assemble(&[item, other]);
        let data = AnimData::parse(&blob).unwrap();
        assert!(data.v2);
        let anim = data.lookup("regular/rt").unwrap();
        assert_eq!(anim.frames.len(), 3);
        assert_eq!(anim.delay, Duration::from_millis(40));
        for (t, rendered) in texts.iter().zip(&anim.frames) {
            assert!(codec::semantic_eq(
                &codec::parse_ansi(t),
                &codec::parse_ansi(rendered)
            ));
        }
        assert_eq!(data.lookup("shiny/zz").unwrap().frames.len(), 1);
        assert!(data.lookup("regular/zz").is_none());
    }

    #[test]
    fn v2_wide_palette_roundtrip() {
        // 条目级并集 >255 色 → wide 位宽 + 首帧全量 + 全未变差分帧
        let mut text = String::new();
        for i in 0..304u16 {
            if i > 0 && i % 16 == 0 {
                text.push('\n');
            }
            text.push_str(&format!("\x1b[38;2;{};{};0m█\x1b[0m", i & 0xff, i >> 8));
        }
        let texts = vec![text.clone(), text.clone()];
        let item = encode_entry("regular/wide", 3, &texts);
        assert!(item.wide);
        let blob = assemble(&[item]);
        let data = AnimData::parse(&blob).unwrap();
        let anim = data.lookup("regular/wide").unwrap();
        assert_eq!(anim.frames.len(), 2);
        for rendered in &anim.frames {
            assert!(codec::semantic_eq(
                &codec::parse_ansi(&text),
                &codec::parse_ansi(rendered)
            ));
        }
    }

    #[test]
    fn v2_lookup_rejects_truncated_delta() {
        // 差分帧字节残缺（变更格被截）→ None 而非 panic
        let texts = vec!["█\n".to_string(), "▀\n".to_string()];
        let mut item = encode_entry("regular/a", 3, &texts);
        let cut = item.body.len() - 6; // 末段尾 (skip,count)=4B + 变更格 3B 中间截断
        item.body.truncate(cut);
        let blob = assemble(&[item]);
        let data = AnimData::parse(&blob).unwrap();
        assert!(data.lookup("regular/a").is_none());
    }

    #[test]
    fn v2_parse_rejects_frame_range_out_of_bounds() {
        let texts = vec!["█\n".to_string()];
        let mut blob = assemble(&[encode_entry("regular/a", 3, &texts)]);
        // pal_n=0：coff 在 p+9..13、clen 在 p+13..17（p = rows 字节位）
        let p = 9 + 1 + "regular/a".len();
        blob[p + 13..p + 17].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(AnimData::parse(&blob).is_none());
    }

    #[test]
    fn v2_parse_rejects_unknown_version() {
        let mut blob = assemble(&[]);
        blob[4] = 9;
        assert!(AnimData::parse(&blob).is_none());
    }

    #[test]
    fn v2_empty_index() {
        let blob = assemble(&[]);
        let data = AnimData::parse(&blob).unwrap();
        assert!(data.v2);
        assert!(data.lookup("regular/a").is_none());
    }
}
