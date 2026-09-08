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

use ruzstd::encoding::{CompressionLevel, compress_to_vec};

// 注意：ruzstd 0.9 编码器只实现了 Uncompressed/Fastest 两档（更高档 unimplemented!()）
const LEVEL: CompressionLevel = CompressionLevel::Fastest;

/// assets/pokemon.json → OUT_DIR/forms_gen.rs 的 static FORMS（name → forms），
/// 供 -f/--form 校验；JSON 只在构建期解析，运行时零开销
fn gen_forms_table(manifest_dir: &Path) {
    println!("cargo:rerun-if-changed=assets/pokemon.json");
    let path = manifest_dir.join("assets/pokemon.json");
    let data: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&path).unwrap_or_else(|e| panic!("读 {} 失败: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("解析 pokemon.json 失败: {e}"));
    let arr = data
        .as_array()
        .unwrap_or_else(|| panic!("pokemon.json 顶层不是数组"));

    let mut out = String::from(
        "// 由 build.rs 从 assets/pokemon.json 生成，勿手改\nstatic FORMS: &[(&str, &[&str])] = &[\n",
    );
    for item in arr {
        let name = item["name"]
            .as_str()
            .unwrap_or_else(|| panic!("pokemon.json 条目缺 name"));
        let forms = item["forms"]
            .as_array()
            .unwrap_or_else(|| panic!("pokemon.json 条目 {name} 缺 forms"))
            .iter()
            .map(|f| {
                f.as_str()
                    .unwrap_or_else(|| panic!("{name} 的 forms 含非字符串"))
            })
            .collect::<Vec<_>>();
        let list = forms
            .iter()
            .map(|f| format!("{f:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("    ({name:?}, &[{list}]),\n"));
    }
    out.push_str("];\n");
    let out_path = Path::new(&std::env::var("OUT_DIR").unwrap()).join("forms_gen.rs");
    fs::write(&out_path, out).unwrap_or_else(|e| panic!("写 {out_path:?} 失败: {e}"));
}

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let assets = manifest_dir.join("assets/colorscripts");
    println!("cargo:rerun-if-changed=assets/colorscripts");
    gen_forms_table(&manifest_dir);

    let mut entries: Vec<(String, Vec<u8>, u8, u8)> = Vec::new();
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
                let (encoded, rows, cols) = encode_file(&key, &text);
                entries.push((key, encoded, rows, cols));
            }
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    // 组 blob：索引区（预留，回头回填偏移）+ 帧区；帧内偏移以帧区起点为 0
    // 索引条目：klen | key | rows u8 | cols u8 | coff u32le | clen u32le
    let mut blob = Vec::new();
    blob.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    let mut index_size = 4usize;
    for (key, _, _, _) in &entries {
        index_size += 1 + key.len() + 10;
    }
    blob.resize(index_size, 0);
    let mut slot = 4usize; // 当前条目在索引区的位置
    for (key, encoded, rows, cols) in &entries {
        let frame = compress_to_vec(encoded.as_slice(), LEVEL);
        verify_frame(key, &frame, encoded);
        let coff = (blob.len() - index_size) as u32;
        let clen = frame.len() as u32;
        blob.extend_from_slice(&frame);
        let slot_end = slot + 1 + key.len() + 10;
        blob[slot] = key.len() as u8;
        blob[slot + 1..slot + 1 + key.len()].copy_from_slice(key.as_bytes());
        blob[slot + 1 + key.len()] = *rows;
        blob[slot + 2 + key.len()] = *cols;
        blob[slot + 3 + key.len()..slot + 7 + key.len()].copy_from_slice(&coff.to_le_bytes());
        blob[slot + 7 + key.len()..slot_end].copy_from_slice(&clen.to_le_bytes());
        slot = slot_end;
    }

    let out_path = Path::new(&std::env::var("OUT_DIR").unwrap()).join("sprites.bin");
    fs::write(&out_path, &blob).unwrap_or_else(|e| panic!("写 {out_path:?} 失败: {e}"));
    eprintln!(
        "pokefetch build: {} 只 × 2 尺寸 × 2 变体，编码 {:.1}MB → 压缩 {:.1}MB",
        entries.len() / 4,
        entries.iter().map(|(_, e, _, _)| e.len()).sum::<usize>() as f64 / 1e6,
        blob.len() as f64 / 1e6
    );
}

/// 单文件：编码 + round-trip 断言（build 期正确性闸门），返回编码字节与可见尺寸
fn encode_file(key: &str, text: &str) -> (Vec<u8>, u8, u8) {
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
    let encoded = codec::encode(&sprite);
    (encoded, sprite.rows as u8, sprite.cols as u8)
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
