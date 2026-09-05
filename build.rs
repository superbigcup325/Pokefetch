// 编译期把 assets/colorscripts 下全部字符画内嵌为静态表：
// 生成 OUT_DIR/sprites_gen.rs，key = "size/variant/name"（按 key 排序，运行时二分查找）
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let assets = manifest_dir.join("assets/colorscripts");
    println!("cargo:rerun-if-changed=assets/colorscripts");

    let mut entries: Vec<(String, String)> = Vec::new();
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
                let content = fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("读文件 {path:?} 失败: {e}"));
                entries.push((key, content));
            }
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out =
        String::from("// 由 build.rs 自动生成，勿手改\nstatic SPRITES: &[(&str, &str)] = &[\n");
    for (key, content) in &entries {
        writeln!(
            out,
            "    (\"{}\", \"{}\"),",
            key.escape_debug(),
            content.escape_debug()
        )
        .unwrap();
    }
    out.push_str("];\n");

    let out_path = Path::new(&std::env::var("OUT_DIR").unwrap()).join("sprites_gen.rs");
    fs::write(&out_path, out).unwrap_or_else(|e| panic!("写 {out_path:?} 失败: {e}"));
}
