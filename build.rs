// 编译期把 assets/colorscripts 全部字符画编码为调色板格子数组（src/codec.rs）：
// - 逐文件 round-trip 断言：encode → render → re-parse 语义比对，任一失败构建即失败
// - 每个精灵独立压缩为 zstd 帧（ruzstd，纯 Rust），压缩后立即解压回比，
//   启动时只需解压命中的那一个帧
// - 产出 OUT_DIR/sprites.bin：n | [klen key coff clen]×n | 压缩帧串联（按 key 排序）
#[path = "src/codec.rs"]
mod codec;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use ruzstd::encoding::{compress_to_vec, CompressionLevel};

// 注意：ruzstd 0.9 编码器只实现了 Uncompressed/Fastest 两档（更高档 unimplemented!()）
const LEVEL: CompressionLevel = CompressionLevel::Fastest;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let assets = manifest_dir.join("assets/colorscripts");
    println!("cargo:rerun-if-changed=assets/colorscripts");

    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for size in ["small", "large"] {
        for variant in ["regular", "shiny"] {
            let dir = assets.join(size).join(variant);
            let mut files: Vec<PathBuf> = fs::read_dir(&dir)
                .unwrap_or_else(|e| panic!("读目录 {dir:?} 失败: {e}"))
                .filter_map(|e| e.ok().map(|e| e.path()))
                .collect();
            files.sort();
            for path in files {
                if !path.is_file() {
                    continue;
                }
                let key = format!(
                    "{size}/{variant}/{}",
                    path.file_name().unwrap().to_string_lossy()
                );
                let text = fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("读文件 {path:?} 失败: {e}"));
                let encoded = encode_file(&key, &text);
                entries.push((key, encoded));
            }
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    // 组 blob：索引区（预留，回头回填偏移）+ 帧区；帧内偏移以帧区起点为 0
    let mut blob = Vec::new();
    blob.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    let mut index_size = 4usize;
    for (key, _) in &entries {
        index_size += 1 + key.len() + 8;
    }
    blob.resize(index_size, 0);
    let mut slot = 4usize; // 当前条目在索引区的位置
    for (key, encoded) in &entries {
        let frame = compress_to_vec(encoded.as_slice(), LEVEL);
        verify_frame(key, &frame, encoded);
        let coff = (blob.len() - index_size) as u32;
        let clen = frame.len() as u32;
        blob.extend_from_slice(&frame);
        let slot_end = slot + 1 + key.len() + 8;
        blob[slot] = key.len() as u8;
        blob[slot + 1..slot + 1 + key.len()].copy_from_slice(key.as_bytes());
        blob[slot + 1 + key.len()..slot + 5 + key.len()].copy_from_slice(&coff.to_le_bytes());
        blob[slot + 5 + key.len()..slot_end].copy_from_slice(&clen.to_le_bytes());
        slot = slot_end;
    }

    let out_path = Path::new(&std::env::var("OUT_DIR").unwrap()).join("sprites.bin");
    fs::write(&out_path, &blob).unwrap_or_else(|e| panic!("写 {out_path:?} 失败: {e}"));
    eprintln!(
        "pokefetch build: {} 只 × 2 尺寸 × 2 变体，编码 {:.1}MB → 压缩 {:.1}MB",
        entries.len() / 4,
        entries.iter().map(|(_, e)| e.len()).sum::<usize>() as f64 / 1e6,
        blob.len() as f64 / 1e6
    );
}

/// 单文件：编码 + round-trip 断言（build 期正确性闸门）
fn encode_file(key: &str, text: &str) -> Vec<u8> {
    let sprite = std::panic::catch_unwind(|| codec::parse_ansi(text))
        .unwrap_or_else(|p| panic!("{key}: 解析失败: {}", panic_msg(p)));
    let mut rendered = String::new();
    codec::decode_and_render(&codec::encode(&sprite), &mut rendered);
    let re = std::panic::catch_unwind(|| codec::parse_ansi(&rendered))
        .unwrap_or_else(|p| panic!("{key}: round-trip 再解析失败: {}", panic_msg(p)));
    assert!(
        codec::semantic_eq(&sprite, &re),
        "{key}: round-trip 语义不一致"
    );
    codec::encode(&sprite)
}

/// 单帧解压回比：编码字节必须无损
fn verify_frame(key: &str, frame: &[u8], expect: &[u8]) {
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(frame)
        .unwrap_or_else(|e| panic!("{key}: 解码器初始化失败: {e}"));
    let mut got = Vec::with_capacity(expect.len());
    decoder
        .read_to_end(&mut got)
        .unwrap_or_else(|e| panic!("{key}: 解码失败: {e}"));
    assert!(got == *expect, "{key}: 压缩帧解压后不一致");
}

fn panic_msg(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "未知 panic".into())
}
