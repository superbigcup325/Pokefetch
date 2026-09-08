// pokefetch —— 终端里的宝可梦 fetch：精灵字符画 + 系统信息面板
// 素材编译期内嵌：build.rs 编码 + 压缩生成 OUT_DIR/sprites.bin（key 排序，运行时二分）
// 编解码核心见 codec.rs（build.rs 与运行时共用）
mod codec;
mod sysinfo;

static BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sprites.bin"));

use std::io::Read;

use ruzstd::decoding::StreamingDecoder;

const NAMES_TXT: &str = include_str!("../assets/names.txt");

/// 随机出 shiny 的分母（1/N，与上游 pokemon-colorscripts 的 1/128 一致）
const SHINY_RATE: u64 = 128;

/// 默认画布宽（列）：small 输出左锚、右垫到该宽度，fastfetch 面板列位由此稳定；
/// large 默认不垫（-b 是刻意行为）。--canvas 可覆盖，0 = 关闭
const DEFAULT_CANVAS: usize = 40;

/// 世代 → 图鉴编号区间（1-based，含端点）；names.txt 行号 = 图鉴编号
const GENERATIONS: [(usize, usize); 8] = [
    (1, 151),
    (152, 251),
    (252, 386),
    (387, 493),
    (494, 649),
    (650, 721),
    (722, 809),
    (810, 898),
];

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

fn die(msg: &str) -> ! {
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

/// "1" / "1-3" / "1,3,6" → 图鉴编号区间列表
fn parse_gens(spec: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    for part in spec.split(',') {
        let (a, b) = part.split_once('-').unwrap_or((part, part));
        let parsed = (a.trim().parse::<usize>(), b.trim().parse::<usize>());
        let (Ok(i), Ok(j)) = parsed else {
            die(&format!("无效世代: {spec}"));
        };
        if !(1..=8).contains(&i) || !(1..=8).contains(&j) || i > j {
            die(&format!("无效世代: {spec}"));
        }
        ranges.push((GENERATIONS[i - 1].0, GENERATIONS[j - 1].1));
    }
    ranges
}

fn help() -> ! {
    eprint!(
        "pokefetch —— 终端里的宝可梦

用法: pokefetch [选项]

  -n, --name <名字>    指定宝可梦（pikachu；形态传全名 charizard-mega-x）
  -r, --random [世代]  随机一只，可选世代: 1 / 1-3 / 1,3,6
  -s, --shiny          强制闪光版（不带时随机有 1/128 概率出 shiny）
  -b, --big            大尺寸字符画（默认 small）
      --canvas <列宽>  画布宽度：左锚右垫到此列宽（0=关闭；默认 small 40、large 不垫）
      --center         精灵在画布内居中（默认左锚）
      --no-panel       只输出精灵，不带系统信息面板
      --title          显示精灵名字行（面板模式下默认不显示）
      --no-title       不显示名字行（纯精灵模式下默认显示）
  -l, --list           列出全部名字

fastfetch 对接:
      --raw            只输出字符画本体（无名字行无面板），可作 logo 源:
                       fastfetch --data-raw \"$(pokefetch -r --raw)\"
  -o, --output <文件>  字符画写入文件（stdout 不输出）
      --logo-cache     写入 ~/.cache/pokefetch/logo.ans 并照常打印；
                       配合仓库附带的 fastfetch.jsonc 使用（fastfetch --config）

随机时自动跳过当前终端放不下的精灵；显式 -n/-b 不做干预、原样输出。
面板信息来自 /proc、/sys 与环境变量，取不到的行自动跳过。
  -h, --help           本帮助
"
    );
    std::process::exit(0);
}

/// 终端尺寸（行, 列）。优先查控制终端 /dev/tty——stdout 被管道接管
/// （fastfetch 注入场景）时它才是最终显示窗口；再退标准流；都不是 tty 返回 None
fn terminal_size() -> Option<(usize, usize)> {
    #[repr(C)]
    struct Winsize {
        rows: u16,
        cols: u16,
        xpix: u16,
        ypix: u16,
    }
    unsafe extern "C" {
        fn open(path: *const std::os::raw::c_char, flags: i32) -> i32;
        fn close(fd: i32) -> i32;
        fn ioctl(fd: i32, request: u64, arg: *mut Winsize) -> i32;
    }
    const TIOCGWINSZ: u64 = 0x5413;
    let query = |fd: i32| unsafe {
        let mut ws = Winsize {
            rows: 0,
            cols: 0,
            xpix: 0,
            ypix: 0,
        };
        if ioctl(fd, TIOCGWINSZ, &mut ws) == 0 && ws.rows > 0 && ws.cols > 0 {
            Some((usize::from(ws.rows), usize::from(ws.cols)))
        } else {
            None
        }
    };
    unsafe {
        let fd = open(c"/dev/tty".as_ptr(), 2 /* O_RDWR */);
        if fd >= 0 {
            let size = query(fd);
            close(fd);
            if size.is_some() {
                return size;
            }
        }
    }
    [1, 0, 2].into_iter().find_map(query)
}

/// 行的可见宽度（字形均单宽；跳过 \x1b[..m 转义段）
fn visible_width(line: &str) -> usize {
    let mut w = 0;
    let mut esc = false;
    for ch in line.chars() {
        if esc {
            if ch == 'm' {
                esc = false;
            }
        } else if ch == '\x1b' {
            esc = true;
        } else {
            w += 1;
        }
    }
    w
}

/// 画布：精灵整体在画布内左锚或居中，每行右垫空格到画布宽
/// （fastfetch 面板列位由此稳定）；行宽已达画布的行不动（精灵超宽时自然伸出，永不裁剪）。
/// 居中按精灵整体最大宽计算统一左偏移，逐行对齐不被打散
fn pad_canvas(ansi: &str, w: usize, center: bool) -> String {
    let max_w = ansi.lines().map(visible_width).max().unwrap_or(0);
    let left = if center {
        w.saturating_sub(max_w) / 2
    } else {
        0
    };
    let mut out = String::with_capacity(ansi.len() + 16);
    for line in ansi.lines() {
        let lw = visible_width(line);
        if left > 0 {
            out.push_str(&" ".repeat(left));
        }
        out.push_str(line);
        let used = left + lw;
        if used < w {
            out.push_str(&" ".repeat(w - used));
        }
        out.push('\n');
    }
    out
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut name: Option<String> = None;
    let mut shiny = false;
    let mut big = false;
    let mut title: Option<bool> = None; // None = 按模式取默认（面板模式隐藏，纯精灵显示）
    let mut random = false;
    let mut gens: Option<String> = None;
    let mut output: Option<std::path::PathBuf> = None;
    let mut logo_cache = false;
    let mut canvas: Option<usize> = None;
    let mut center = false;
    let mut no_panel = false;
    let mut raw = false;

    let mut it = args.iter().peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => help(),
            "-l" | "--list" => {
                for n in names() {
                    println!("{n}");
                }
                return;
            }
            "-n" | "--name" => match it.next() {
                Some(v) => name = Some(v.clone()),
                None => die("-n 需要一个名字"),
            },
            "-r" | "--random" => {
                random = true;
                if let Some(v) =
                    it.next_if(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
                {
                    gens = Some(v.clone());
                }
            }
            "-s" | "--shiny" => shiny = true,
            "-b" | "--big" => big = true,
            "--canvas" => match it.next() {
                Some(v) => match v.parse::<usize>() {
                    Ok(n) if n <= 1000 => canvas = Some(n),
                    _ => die("--canvas 需要一个 0~1000 的整数"),
                },
                None => die("--canvas 需要一个列宽"),
            },
            "--center" => center = true,
            "--title" => title = Some(true),
            "--no-title" => title = Some(false),
            "--no-panel" => no_panel = true,
            "--raw" => raw = true,
            "-o" | "--output" => match it.next() {
                Some(v) => output = Some(std::path::PathBuf::from(v)),
                None => die("-o 需要一个文件路径"),
            },
            "--logo-cache" => {
                output = Some(cache_logo_path());
                logo_cache = true;
            }
            other => die(&format!("未知参数: {other}（-h 看用法）")),
        }
    }

    let index = sprites_index();

    // 终端检测只服务两件事：随机池过滤 + 默认画布收窄。
    // 上屏路径（stdout / --logo-cache）才检测；裸 -o 写文件保持确定性
    let adapt = output.is_none() || logo_cache;
    let term = if adapt { terminal_size() } else { None };

    let mut rng = Rng::new();
    let chosen: String = match name {
        // 显式 -n 且未要求随机：直接用
        Some(n) if !random => n,
        // 随机路径（无 -n，或 -r 覆盖显式名字）
        _ => {
            // shiny 先定（分布与顺序无关），池子按该变体的宽度过滤才有意义
            if !shiny {
                shiny = rng.next_u64().is_multiple_of(SHINY_RATE);
            }
            let size = if big { "large" } else { "small" };
            let variant = if shiny { "shiny" } else { "regular" };
            let (lo, hi) = match &gens {
                Some(spec) => {
                    let ranges = parse_gens(spec);
                    ranges[rng.below(ranges.len())]
                }
                None => (1, names().len()),
            };
            let mut pool: Vec<&'static str> = names()[lo - 1..hi].to_vec();
            // 只 roll 终端放得下的精灵；极端窄终端全放不下时放弃过滤兜底
            if let Some((_, cols)) = term {
                let fits: Vec<&'static str> = pool
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

    // 画布：显式 --canvas 原样生效（两尺寸都垫、不随终端收窄）；
    // 默认 small=40 并随终端收窄，large 不垫
    let canvas_w = match canvas {
        Some(0) => 0,
        Some(n) => n,
        None => {
            if big {
                0
            } else {
                match term {
                    Some((_, cols)) => DEFAULT_CANVAS.min(cols),
                    None => DEFAULT_CANVAS,
                }
            }
        }
    };
    let ansi = if canvas_w > 0 {
        pad_canvas(&ansi, canvas_w, center)
    } else {
        ansi
    };

    // 面板只挂 stdout 直打印路径；--raw / 写文件（含 --logo-cache）恒为纯精灵
    let panel = output.is_none() && !no_panel && !raw;
    let show_title = title.unwrap_or(!panel && !raw);

    match &output {
        Some(path) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .unwrap_or_else(|e| die(&format!("建目录 {parent:?} 失败: {e}")));
            }
            std::fs::write(path, &ansi).unwrap_or_else(|e| die(&format!("写 {path:?} 失败: {e}")));
            if logo_cache {
                // 缓存模式照常打印，让用户知道这次抽到了谁
                if show_title {
                    println!("{chosen}{}", if shiny { " (shiny)" } else { "" });
                }
                print!("{ansi}");
            }
        }
        None => {
            if show_title {
                println!("{chosen}{}", if shiny { " (shiny)" } else { "" });
            }
            let body = if panel {
                let info = sysinfo::collect();
                let sprite_w = ansi.lines().map(visible_width).max().unwrap_or(0);
                compose(&ansi, panel_rows(&info), sprite_w, 3)
            } else {
                ansi
            };
            print!("{body}");
        }
    }
}

/// 面板行：user@host 标题、分隔线、蓝色 key 的 key:value、空行、两行色块
fn panel_rows(info: &sysinfo::FetchInfo) -> Vec<String> {
    let mut rows = vec![
        format!(
            "\x1b[1;32m{}\x1b[0m@\x1b[1;34m{}\x1b[0m",
            info.user, info.host
        ),
        "-".repeat(info.user.chars().count() + 1 + info.host.chars().count()),
    ];
    for (k, v) in &info.rows {
        rows.push(format!("\x1b[1;34m{k}:\x1b[0m {v}"));
    }
    rows.push(String::new());
    for base in [30u8, 90] {
        let blocks: Vec<String> = (0..8u8)
            .map(|i| format!("\x1b[{base}m███\x1b[0m", base = base + i))
            .collect();
        rows.push(blocks.join(" "));
    }
    rows
}

/// 精灵与面板逐行拼接：精灵侧统一垫到 sprite_w，矮的一侧自然延续到末尾
fn compose(sprite: &str, panel: Vec<String>, sprite_w: usize, gap: usize) -> String {
    let sprite_lines: Vec<&str> = sprite.lines().collect();
    let mut out = String::new();
    for i in 0..sprite_lines.len().max(panel.len()) {
        let left = match sprite_lines.get(i) {
            Some(l) => {
                let lw = visible_width(l);
                let mut s = String::with_capacity(sprite_w);
                s.push_str(l);
                if lw < sprite_w {
                    s.push_str(&" ".repeat(sprite_w - lw));
                }
                s
            }
            None => " ".repeat(sprite_w),
        };
        out.push_str(&left);
        if let Some(p) = panel.get(i) {
            out.push_str(&" ".repeat(gap));
            out.push_str(p);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_width_skips_escapes() {
        assert_eq!(visible_width("ab\x1b[38;2;1;2;3mcd\x1b[0m"), 4);
        assert_eq!(visible_width(""), 0);
    }

    #[test]
    fn pad_left_anchors_right_pad() {
        assert_eq!(pad_canvas("█\n██\n", 4, false), "█   \n██  \n");
    }

    #[test]
    fn pad_centers_with_uniform_offset() {
        // 精灵最大宽 2，画布 5：统一左偏移 1，逐行右垫到 5
        assert_eq!(pad_canvas("█\n██\n", 5, true), " █   \n ██  \n");
    }

    #[test]
    fn pad_skips_wide_lines() {
        // 行宽已达画布：不垫（精灵超宽自然伸出）
        assert_eq!(pad_canvas("████\n", 2, false), "████\n");
        assert_eq!(pad_canvas("████\n", 2, true), "████\n");
    }

    #[test]
    fn pad_zero_is_noop() {
        assert_eq!(pad_canvas("█\n", 0, false), "█\n");
        assert_eq!(pad_canvas("█\n", 0, true), "█\n");
    }

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
    fn compose_extends_shorter_side() {
        let panel = vec![
            "OS: x".to_string(),
            "Kernel: y".to_string(),
            "Uptime: z".to_string(),
        ];
        let out = compose("aa\nbb\n", panel, 2, 2);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines, ["aa  OS: x", "bb  Kernel: y", "    Uptime: z"]);
    }

    #[test]
    fn compose_sprite_taller() {
        let panel = vec!["OS: x".to_string()];
        let out = compose("aa\nbb\n", panel, 2, 2);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines, ["aa  OS: x", "bb"]);
    }

    #[test]
    fn panel_structure() {
        let info = sysinfo::FetchInfo {
            user: "u".into(),
            host: "h".into(),
            rows: vec![("OS".into(), "x".into())],
        };
        let rows = panel_rows(&info);
        assert_eq!(rows.len(), 6); // title, 分隔线, kv, 空行, 色块×2
        assert!(rows[0].starts_with("\x1b[1;32mu\x1b[0m@\x1b[1;34mh"));
        assert_eq!(rows[1], "---");
        assert!(rows[2].starts_with("\x1b[1;34mOS:"));
        assert!(rows[5].contains("███"));
    }
}
