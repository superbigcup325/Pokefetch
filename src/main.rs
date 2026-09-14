// pokefetch —— 终端里的宝可梦 fetch：精灵字符画 + 系统信息面板
// 素材编译期内嵌：build.rs 编码 + 压缩生成 OUT_DIR/sprites.bin（key 排序，运行时二分）
// 编解码核心见 codec.rs（build.rs 与运行时共用）；
// CLI 表面在 cli.rs，画布几何在 canvas.rs，面板渲染在 panel.rs，
// 动画获取与播放循环在 play.rs，系统信息数据层在 sysinfo/（注册表 + 数据域子模块）
mod anim;
mod canvas;
mod cli;
mod codec;
mod ffi;
mod panel;
mod play;
mod sysinfo;

static BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sprites.bin"));

// -f/--form 的形态表：build.rs 从 assets/pokemon.json 生成
include!(concat!(env!("OUT_DIR"), "/forms_gen.rs"));

use std::io::Read;

use canvas::{compose, max_visible_width, pad_canvas, resolve_canvas};
use cli::Args;
use ruzstd::decoding::StreamingDecoder;

const NAMES_TXT: &str = include_str!("../assets/names.txt");

/// 随机出 shiny 的分母（1/N，与上游 pokemon-colorscripts 的 1/128 一致）
const SHINY_RATE: u64 = 128;

struct Rng(u64);

impl Rng {
    fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Rng(seed | 1)
    }

    /// xorshift64，够 fetch 用
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

fn names() -> Vec<&'static str> {
    NAMES_TXT.lines().collect()
}

pub(crate) fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

/// 索引条目：key / 可见列宽 / 帧偏移（绝对）/ 帧长
type Entry<'a> = (&'a str, usize, usize, usize);

/// 启动时解析 blob 索引：key 按构建期排序，供二分查找
fn sprites_index() -> Vec<Entry<'static>> {
    let n = u32::from_le_bytes(BLOB[..4].try_into().unwrap()) as usize;
    let mut out = Vec::with_capacity(n);
    let mut pos = 4;
    for _ in 0..n {
        let klen = BLOB[pos] as usize;
        let key = std::str::from_utf8(&BLOB[pos + 1..pos + 1 + klen]).unwrap();
        let dims = pos + 1 + klen; // rows u8 | cols u8
        let coff = u32::from_le_bytes(BLOB[dims + 2..dims + 6].try_into().unwrap()) as usize;
        let clen = u32::from_le_bytes(BLOB[dims + 6..dims + 10].try_into().unwrap()) as usize;
        out.push((key, BLOB[dims + 1] as usize, coff, clen));
        pos = dims + 10;
    }
    let frames_start = pos; // 帧区紧跟索引区；coff 以帧区起点为 0
    out.iter_mut()
        .for_each(|(_, _, coff, _)| *coff += frames_start);
    out
}

/// 指定 size/variant 下该精灵的可见列宽（索引查不到返回 None）
fn sprite_cols(index: &[Entry<'static>], size: &str, variant: &str, name: &str) -> Option<usize> {
    let key = format!("{size}/{variant}/{name}");
    index
        .binary_search_by(|(k, _, _, _)| (*k).cmp(key.as_str()))
        .ok()
        .map(|i| index[i].1)
}

/// 取一只精灵的 ANSI 文本（未命中即 die）
fn sprite_ansi(index: &[Entry<'static>], name: &str, shiny: bool, big: bool) -> String {
    let size = if big { "large" } else { "small" };
    let variant = if shiny { "shiny" } else { "regular" };
    let key = format!("{size}/{variant}/{name}");
    match index.binary_search_by(|(k, _, _, _)| (*k).cmp(key.as_str())) {
        Ok(i) => {
            let (_, _, coff, clen) = index[i];
            let frame = &BLOB[coff..coff + clen];
            let mut decoder = StreamingDecoder::new(frame)
                .unwrap_or_else(|e| die(&format!("内部错误: 解码器初始化失败: {e}")));
            let mut encoded = Vec::new();
            decoder
                .read_to_end(&mut encoded)
                .unwrap_or_else(|e| die(&format!("内部错误: 解码失败: {e}")));
            let mut ansi = String::new();
            codec::decode_and_render(&encoded, &mut ansi);
            ansi
        }
        Err(_) => die(&format!(
            "没有这只宝可梦: {name}（pokefetch -l 查列表，形态直接传全名如 charizard-mega-x）"
        )),
    }
}

/// -f/--form：校验形态存在，拼出素材名（regular 即本名）
fn resolve_form(name: &str, form: &str) -> String {
    let Some(forms) = FORMS.iter().find(|(n, _)| *n == name).map(|(_, f)| *f) else {
        die(&format!("{name} 不在形态表中"));
    };
    if !forms.contains(&form) {
        die(&format!(
            "{name} 没有形态 {form}（可用: {}）",
            forms.join(", ")
        ));
    }
    if form == "regular" {
        name.to_string()
    } else {
        format!("{name}-{form}")
    }
}

/// 图鉴编号 = names.txt 行号（1-based）；形态名回退到基础名
/// （charizard-mega-x → charizard；ho-oh / porygon-z 这类原生带连字符的不受影响）
pub(crate) fn dex_number(name: &str) -> Option<usize> {
    let all = names();
    let mut cur = name;
    loop {
        if let Some(i) = all.iter().position(|&n| n == cur) {
            return Some(i + 1);
        }
        let (base, _) = cur.rsplit_once('-')?;
        cur = base;
    }
}

/// 默认 logo 缓存路径：$XDG_CACHE_HOME/pokefetch/logo.ans
fn cache_logo_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| die("HOME 未设置"));
            std::path::PathBuf::from(home).join(".cache")
        });
    base.join("pokefetch").join("logo.ans")
}

/// 输出目的地：stdout 直打印（可挂面板/标题），或写文件
/// （--logo-cache 写完照常回显，让用户知道这次抽到了谁）
enum Dest {
    Stdout,
    File {
        path: std::path::PathBuf,
        echo: bool,
    },
}

fn main() {
    let args = cli::parse_args();

    // 维护命令：帧目录 → anim.bin（与常规输出互斥，打完即走）
    if let Some(dir) = &args.anim_pack {
        let Some(out) = &args.output else {
            die("--anim-pack 需要 -o 指定输出文件，如 -o ~/.local/share/pokefetch/anim.bin");
        };
        anim::pack(dir, out);
        return;
    }

    if args.list {
        for n in names() {
            println!("{n}");
        }
        return;
    }
    if args.canvas.is_some_and(|c| c > 1000) {
        die("--canvas 需要一个 0~1000 的整数");
    }

    let Args {
        name,
        random,
        random_by_names,
        form,
        mut shiny,
        big,
        canvas,
        center,
        no_panel,
        modules,
        title,
        no_title,
        raw,
        output,
        logo_cache,
        animated,
        loops,
        anim_pack: _,
        list: _,
    } = args;

    let title = if no_title {
        Some(false)
    } else if title {
        Some(true)
    } else {
        None
    };
    // -r 不带值时 default_missing_value 为空串
    let random_given = random.is_some();
    let gens = match random.as_deref() {
        Some("") | None => None,
        Some(spec) => Some(spec.to_string()),
    };
    let by_names: Option<Vec<String>> = random_by_names.as_deref().map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    });
    let modules: Option<Vec<String>> = modules.map(|s| {
        let list: Vec<String> = s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect();
        if list.is_empty() {
            die("--modules 需要模块列表，如 os,gpu,memory");
        }
        list
    });

    let index = sprites_index();

    // --logo-cache 写缓存路径（显式 -o 优先）；echo 让用户看到抽到了谁
    let output = if logo_cache {
        Some(output.unwrap_or_else(cache_logo_path))
    } else {
        output
    };
    let dest = match output {
        Some(path) => Dest::File {
            path,
            echo: logo_cache,
        },
        None => Dest::Stdout,
    };
    let on_stdout = matches!(dest, Dest::Stdout);
    let echo = matches!(dest, Dest::File { echo: true, .. });

    // 终端检测只服务两件事：随机池过滤 + 默认画布收窄。
    // 上屏路径（stdout / --logo-cache）才检测；裸 -o 写文件保持确定性
    let term = if on_stdout || echo {
        ffi::terminal_size()
    } else {
        None
    };

    let mut rng = Rng::new();
    let chosen: String = match name {
        // 显式 -n 且未要求随机：直接用（-f 拼形态）
        Some(n) if !random_given => match &form {
            Some(f) => resolve_form(&n, f),
            None => n,
        },
        // 随机路径（无 -n，或 -r 覆盖显式名字）
        _ => {
            // shiny 先定（分布与顺序无关），池子按该变体的宽度过滤才有意义
            if !shiny {
                shiny = rng.next_u64().is_multiple_of(SHINY_RATE);
            }
            let size = if big { "large" } else { "small" };
            let variant = if shiny { "shiny" } else { "regular" };
            let all = names();
            let mut pool: Vec<&str> = if let Some(list) = &by_names {
                // 校验走索引键（与 -n 同口径，形态全名可用），不只限 names.txt 基础名
                let picked: Vec<&str> = list.iter().map(String::as_str).collect();
                for n in &picked {
                    if sprite_cols(&index, size, variant, n).is_none() {
                        die(&format!("没有这只宝可梦: {n}"));
                    }
                }
                picked
            } else {
                let (lo, hi) = match &gens {
                    Some(spec) => {
                        let ranges = cli::parse_gens(spec);
                        ranges[rng.below(ranges.len())]
                    }
                    None => (1, all.len()),
                };
                all[lo - 1..hi].to_vec()
            };
            // 只 roll 终端放得下的精灵；极端窄终端全放不下时放弃过滤兜底
            if let Some((_, cols)) = term {
                let fits: Vec<&str> = pool
                    .iter()
                    .copied()
                    .filter(|n| sprite_cols(&index, size, variant, n).is_some_and(|w| w <= cols))
                    .collect();
                if !fits.is_empty() {
                    pool = fits;
                }
            }
            pool[rng.below(pool.len())].to_owned()
        }
    };

    let ansi = sprite_ansi(&index, &chosen, shiny, big);

    // 动画路径：--animated 且 stdout 直连终端且数据命中；任一不满足回退静态图。
    // fastfetch 对接路径（--raw/-o/--logo-cache）被 clap 互斥挡住，恒为静态
    let animation = play::prepare(animated, shiny, &chosen);

    // 画布：显式 --canvas 原样生效（两尺寸都垫、不随终端收窄）；
    // 默认 small=40 并随终端收窄，large 不垫
    let canvas_w = resolve_canvas(canvas, big, term.map(|(_, cols)| cols));
    // 最大可见行宽全程只扫一次：垫宽用（居中偏移），精灵区宽由它派生——
    // 垫宽后 = max(原宽, 画布宽)（超宽行不裁、原样伸出）；
    // 动画按全帧联合最大宽算统一偏移，帧间不抖
    let raw_w = match &animation {
        Some(a) => a
            .frames
            .iter()
            .map(|f| max_visible_width(f))
            .max()
            .unwrap_or(0),
        None => max_visible_width(&ansi),
    };
    let pad = |s: &str| {
        if canvas_w > 0 {
            pad_canvas(s, canvas_w, center, raw_w)
        } else {
            s.to_string()
        }
    };
    let rendered: Vec<String> = match &animation {
        Some(a) => a.frames.iter().map(|f| pad(f)).collect(),
        None => vec![pad(&ansi)],
    };
    let sprite_w = raw_w.max(canvas_w);

    // 面板只挂 stdout 直打印路径；--raw / 写文件（含 --logo-cache）恒为纯精灵
    let panel = on_stdout && !no_panel && !raw;
    // 名字行：--raw 恒隐藏；stdout 面板模式默认隐藏；纯精灵 stdout 与写文件
    // 回显默认显示；--title/--no-title 显式覆盖
    let show_title = title.unwrap_or(!raw && !(on_stdout && panel));

    // 面板行只收集一次，静态打印与动画逐帧复用（动画时面板保持静态）
    let panel_rows_v = if panel {
        let names = sysinfo::resolve(modules.as_deref()).unwrap_or_else(|e| die(&e));
        let info = sysinfo::collect(&names);
        // 行宽预算：默认集在窄终端下逐行截值防折行（显式 --modules 硬打不裁）；
        // 面板起点 = 精灵区宽 + gap，无终端检测（非 tty）则不裁（确定性）。
        // 预算 0 = 终端只够精灵区，面板整体让位（gap 残行也会溢出）
        let budget = panel::row_budget(modules.is_none(), term.map(|(_, cols)| cols), sprite_w);
        match budget {
            Some(0) => None,
            b => {
                // 图鉴编号行：names.txt 行号即编号，与其他行同受预算约束
                let dex = dex_number(&chosen).map(|num| {
                    format!(
                        "\x1b[1;34mDex:\x1b[0m #{num:03}{}",
                        if shiny { " ✨" } else { "" }
                    )
                });
                Some(panel::panel_rows(&info, b, dex))
            }
        }
    } else {
        None
    };

    if show_title && (on_stdout || echo) {
        println!("{chosen}{}", if shiny { " (shiny)" } else { "" });
    }
    match dest {
        Dest::Stdout => match &animation {
            Some(a) => play::run(a, &rendered, panel_rows_v.as_deref(), sprite_w, loops),
            None => {
                let body = match panel_rows_v {
                    Some(rows) => compose(&rendered[0], rows, sprite_w, 3),
                    None => rendered[0].clone(),
                };
                print!("{body}");
            }
        },
        Dest::File { path, echo } => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .unwrap_or_else(|e| die(&format!("建目录 {parent:?} 失败: {e}")));
            }
            std::fs::write(&path, &ansi).unwrap_or_else(|e| die(&format!("写 {path:?} 失败: {e}")));
            if echo {
                print!("{ansi}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_has_known_sprites() {
        let index = sprites_index();
        for key in ["small/regular/pikachu", "large/shiny/charizard-mega-x"] {
            let i = index
                .binary_search_by(|(k, _, _, _)| (*k).cmp(key))
                .unwrap_or_else(|_| panic!("{key} 应在索引中"));
            let cols = index[i].1;
            assert!(cols > 0, "{key} 列宽非法");
        }
    }

    #[test]
    fn sprite_cols_lookup() {
        let index = sprites_index();
        let pikachu = sprite_cols(&index, "small", "regular", "pikachu");
        assert!(pikachu.is_some_and(|w| (1..=68).contains(&w)));
        assert_eq!(sprite_cols(&index, "small", "regular", "不存在"), None);
    }

    #[test]
    fn dex_numbers() {
        assert_eq!(dex_number("pikachu"), Some(25));
        assert_eq!(dex_number("charizard"), Some(6));
        // 形态回退基础名
        assert_eq!(dex_number("charizard-mega-x"), Some(6));
        // 原生带连字符的名字直接命中，不被误拆
        assert_eq!(dex_number("ho-oh"), Some(250));
        assert_eq!(dex_number("porygon-z"), Some(474));
        assert_eq!(dex_number("不存在"), None);
    }
}
