// 编译期把 assets/colorscripts 全部字符画编码为调色板格子数组（src/codec.rs）：
// - 逐文件 round-trip 断言：encode → render → re-parse 语义比对，任一失败构建即失败
// - 每个精灵独立压缩为 zstd 帧（ruzstd，纯 Rust），压缩后立即解压回比，
//   启动时只需解压命中的那一个帧
// - 产出 OUT_DIR/sprites.bin：n | [klen key coff clen]×n | 压缩帧串联（按 key 排序）
#[path = "src/codec.rs"]
mod codec;

use std::collections::HashSet;
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

/// -r 随机的平行形态判定：模拟游戏实际刷新——野外可自然遭遇的永久平行
/// 形态（地区/季节/天气/花色等）参与组内均分；mega/gmax/primal 等战斗临时
/// 形态、道具/剧情切换形态、活动分发形态一律不参与。规则集中于此，调整口径改这里
fn parallel_form(name: &str, form: &str) -> bool {
    let segs: Vec<&str> = form.split('-').collect();
    let has = |s: &str| segs.contains(&s);
    // 战斗临时形态：mega 全家（x/y/z 及组合段）、极巨化、原始回归、禅定变身；
    // Boss/事件/活动专属：noble（洗翠首领）、floette-eternal（永唤之花不可获）、
    // eternamax、cap（帽子皮卡丘等活动分发）
    if has("mega")
        || has("gmax")
        || has("primal")
        || has("zen")
        || has("noble")
        || has("cap")
        || form == "eternal"
        || form == "eternamax"
    {
        return false;
    }
    // 地区形态：阿罗拉/伽勒尔/洗翠/帕底亚（含组合段如 tauros-paldea-combat-breed）
    if segs
        .iter()
        .any(|s| matches!(*s, "alola" | "galar" | "hisui" | "paldea"))
    {
        return true;
    }
    match name {
        // 季节形态（BW2 季节轮换下野外直遇）
        "deerling" | "sawsbuck" => matches!(form, "autumn" | "summer" | "winter"),
        // 天气形态（SWSH 天气区野外直遇）
        "castform" => matches!(form, "rainy" | "sunny" | "snowy"),
        // 遗迹字母（各字母独立遭遇）
        "unown" => true,
        // 花色（花田分布）
        "flabebe" | "floette" | "florges" => matches!(form, "blue" | "orange" | "white" | "yellow"),
        // 尺寸（野外按概率可遇，均分近似）
        "pumpkaboo" => matches!(form, "small" | "large" | "super"),
        // 东西海（分布按半区）
        "shellos" | "gastrodon" => form == "east",
        // 条纹（不同水域）
        "basculin" => matches!(form, "blue-striped" | "white-striped"),
        // 披风（甜甜蜜树随机）
        "burmy" | "wormadam" => matches!(form, "sandy" | "trash"),
        // 岛屿风格（按岛屿分布）
        "oricorio" => true,
        // 昼夜（USUM 按时间可遇）
        "lycanroc" => matches!(form, "dusk" | "midnight"),
        // 三色（帕底亚按颜色区）
        "tatsugiri" => matches!(form, "droopy" | "stretchy"),
        // 羽色（按城区分布）
        "squawkabilly" => form.ends_with("plumage"),
        // 性格/进化定型的固有分支
        "toxtricity" => form == "low-key",
        "dudunsparce" => form == "three-segment",
        "maushold" => form == "family-of-three",
        // 性别形态（素材单列性别文件的：basculegion/oinkologne）
        _ => has("female"),
    }
}

/// pokemon.json + parallel_form 规则 → OUT_DIR/parallel_forms_gen.rs 的
/// static PARALLEL_FORMS（基础名 → 参与随机的平行形态全名，两侧均排序）。
/// 参与形态要求 2 尺寸 × 2 变体素材齐备，缺一构建即失败
fn gen_parallel_forms_table(manifest_dir: &Path, asset_keys: &HashSet<String>) {
    let path = manifest_dir.join("assets/pokemon.json");
    let data: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&path).unwrap_or_else(|e| panic!("读 {} 失败: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("解析 pokemon.json 失败: {e}"));
    let arr = data
        .as_array()
        .unwrap_or_else(|| panic!("pokemon.json 顶层不是数组"));

    let mut rows: Vec<(String, Vec<String>)> = Vec::new();
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
            });
        let mut picked: Vec<String> = Vec::new();
        for form in forms {
            if form == "regular" || !parallel_form(name, form) {
                continue;
            }
            let full = format!("{name}-{form}");
            for size in ["small", "large"] {
                for variant in ["regular", "shiny"] {
                    let key = format!("{size}/{variant}/{full}");
                    assert!(asset_keys.contains(&key), "参与随机的平行形态缺素材: {key}");
                }
            }
            picked.push(full);
        }
        if !picked.is_empty() {
            picked.sort();
            rows.push((name.to_owned(), picked));
        }
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = String::from(
        "// 由 build.rs 从 assets/pokemon.json + 随机平行形态规则生成，勿手改\nstatic PARALLEL_FORMS: &[(&str, &[&str])] = &[\n",
    );
    for (name, forms) in &rows {
        let list = forms
            .iter()
            .map(|f| format!("{f:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("    ({name:?}, &[{list}]),\n"));
    }
    out.push_str("];\n");
    let out_path = Path::new(&std::env::var("OUT_DIR").unwrap()).join("parallel_forms_gen.rs");
    fs::write(&out_path, out).unwrap_or_else(|e| panic!("写 {out_path:?} 失败: {e}"));
    eprintln!("pokefetch build: 平行形态随机表 {} 项", rows.len());
}

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let assets = manifest_dir.join("assets/colorscripts");
    println!("cargo:rerun-if-changed=assets/colorscripts");

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

    // 平行形态随机表：素材扫描完成后生成，用素材 key 集合校验四组合齐备
    gen_forms_table(&manifest_dir);
    let asset_keys: HashSet<String> = entries.iter().map(|(k, _, _, _)| k.clone()).collect();
    gen_parallel_forms_table(&manifest_dir, &asset_keys);

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
