// pokefetch —— 随机/指定打印宝可梦字符画
// 素材编译期内嵌：build.rs 编码 + 压缩生成 OUT_DIR/sprites.bin（key 排序，运行时二分）
// 编解码核心见 codec.rs（build.rs 与运行时共用）
mod codec;

static BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sprites.bin"));

use std::io::Read;

use ruzstd::decoding::StreamingDecoder;

const NAMES_TXT: &str = include_str!("../assets/names.txt");

/// 随机出 shiny 的分母（1/N，与上游 pokemon-colorscripts 的 1/128 一致）
const SHINY_RATE: u64 = 128;

/// 输出画布（行, 列）＝ 素材尺寸 p99 分位，只垫空格/空行居中、永不裁剪。
/// 固定画布让 fastfetch 的信息面板列位稳定，不随精灵胖瘦跳动
const CANVAS_SMALL: (usize, usize) = (26, 52);
const CANVAS_LARGE: (usize, usize) = (52, 104);

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

/// 启动时解析 blob 索引：key 按构建期排序，供二分查找
fn sprites_index() -> Vec<(&'static str, usize, usize)> {
    let n = u32::from_le_bytes(BLOB[..4].try_into().unwrap()) as usize;
    let mut out = Vec::with_capacity(n);
    let mut pos = 4;
    for _ in 0..n {
        let klen = BLOB[pos] as usize;
        let key = std::str::from_utf8(&BLOB[pos + 1..pos + 1 + klen]).unwrap();
        let coff = u32::from_le_bytes(BLOB[pos + 1 + klen..pos + 5 + klen].try_into().unwrap())
            as usize;
        let clen =
            u32::from_le_bytes(BLOB[pos + 5 + klen..pos + 9 + klen].try_into().unwrap()) as usize;
        out.push((key, coff, clen));
        pos += 1 + klen + 8;
    }
    let frames_start = pos; // 帧区紧跟索引区；coff 以帧区起点为 0
    out.iter_mut().for_each(|(_, coff, _)| *coff += frames_start);
    out
}

/// 取一只精灵的 ANSI 文本（未命中即 die）
fn sprite_ansi(index: &[(&'static str, usize, usize)], name: &str, shiny: bool, big: bool) -> String {
    let size = if big { "large" } else { "small" };
    let variant = if shiny { "shiny" } else { "regular" };
    let key = format!("{size}/{variant}/{name}");
    match index.binary_search_by(|(k, _, _)| (*k).cmp(key.as_str())) {
        Ok(i) => {
            let (_, coff, clen) = index[i];
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
      --no-title       不显示名字行
  -l, --list           列出全部名字

fastfetch 对接:
      --raw            只输出字符画本体（无名字行），可作 logo 源:
                       fastfetch --data-raw \"$(pokefetch -r --raw)\"
  -o, --output <文件>  字符画写入文件（stdout 不输出，不做终端适配）
      --logo-cache     写入 ~/.cache/pokefetch/logo.ans 并照常打印；
                       配合仓库附带的 fastfetch.jsonc 使用（fastfetch --config）

输出按固定画布（small 26×52 / large 52×104）居中并统一适配终端：
画布随终端收窄，large 放不下自动降级 small（stderr 提示）。
  -h, --help           本帮助
"
    );
    std::process::exit(0);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut name: Option<String> = None;
    let mut shiny = false;
    let mut big = false;
    let mut title = true;
    let mut random = false;
    let mut gens: Option<String> = None;
    let mut output: Option<std::path::PathBuf> = None;
    let mut logo_cache = false;

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
            "--no-title" => title = false,
            "--raw" => title = false,
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

    let mut rng = Rng::new();
    let chosen: String = if random || name.is_none() {
        let pool: Vec<&'static str> = match &gens {
            Some(spec) => {
                let ranges = parse_gens(spec);
                let (lo, hi) = ranges[rng.below(ranges.len())];
                names()[lo - 1..hi].to_vec()
            }
            None => names(),
        };
        let picked = pool[rng.below(pool.len())];
        if !shiny {
            shiny = rng.next_u64() % SHINY_RATE == 0;
        }
        picked.to_owned()
    } else {
        name.unwrap()
    };

    let index = sprites_index();

    // 统一适配判定：stdout（含 --raw 注入 fastfetch）与 --logo-cache 的消费端
    // 都是终端，走同一套检测+画布+降级；裸 -o 写文件要求确定性，不适配
    let adapt = output.is_none() || logo_cache;
    let term = if adapt { terminal_size() } else { None };

    let mut big = big;
    let mut ansi = sprite_ansi(&index, &chosen, shiny, big);
    if let Some((_, cols)) = term {
        let (_, w) = ansi_dims(&ansi);
        if big && w > cols {
            eprintln!("提示: 终端宽 {cols} 列放不下 large（需 {w} 列），已改用 small");
            big = false;
            ansi = sprite_ansi(&index, &chosen, shiny, big);
        }
    }

    let std_canvas = if big { CANVAS_LARGE } else { CANVAS_SMALL };
    let canvas = term
        .map_or(std_canvas, |(rows, cols)| {
            (std_canvas.0.min(rows), std_canvas.1.min(cols))
        });
    let ansi = pad_canvas(&ansi, canvas);

    match &output {
        Some(path) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .unwrap_or_else(|e| die(&format!("建目录 {parent:?} 失败: {e}")));
            }
            std::fs::write(path, &ansi)
                .unwrap_or_else(|e| die(&format!("写 {path:?} 失败: {e}")));
            if logo_cache {
                // 缓存模式照常打印，让用户知道这次抽到了谁
                if title {
                    println!("{chosen}{}", if shiny { " (shiny)" } else { "" });
                }
                print!("{ansi}");
            }
        }
        None => {
            if title {
                println!("{chosen}{}", if shiny { " (shiny)" } else { "" });
            }
            print!("{ansi}");
        }
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

// ---- 终端适配（fastfetch 注入与直接打印统一走同一判定） ----

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
        let mut ws = Winsize { rows: 0, cols: 0, xpix: 0, ypix: 0 };
        ((ioctl(fd, TIOCGWINSZ, &mut ws) == 0) && ws.rows > 0 && ws.cols > 0)
            .then(|| (ws.rows as usize, ws.cols as usize))
    };
    unsafe {
        let fd = open(b"/dev/tty\0".as_ptr() as *const _, 2 /* O_RDWR */);
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

/// 渲染产物的可见尺寸（行, 列）
fn ansi_dims(ansi: &str) -> (usize, usize) {
    let mut rows = 0;
    let mut cols = 0;
    for line in ansi.split('\n') {
        rows += 1;
        cols = cols.max(visible_width(line));
    }
    if ansi.ends_with('\n') {
        rows -= 1; // 末尾换行的空行不算
    }
    (rows, cols)
}

/// 把精灵整体居中进画布：左右垫到统一画布宽（fastfetch 面板列位由此稳定），
/// 上下垫空行；精灵在某个方向已达/超过画布时该方向不垫（溢出伸出去，不裁剪）
fn pad_canvas(ansi: &str, canvas: (usize, usize)) -> String {
    let mut lines: Vec<&str> = ansi.split('\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let h = lines.len();
    let w = lines.iter().map(|l| visible_width(l)).max().unwrap_or(0);
    let top = canvas.0.saturating_sub(h) / 2;
    let left = canvas.1.saturating_sub(w) / 2;

    let mut out = String::new();
    out.push_str(&"\n".repeat(top));
    for line in &lines {
        let lw = visible_width(line);
        out.push_str(&" ".repeat(left));
        out.push_str(line);
        let used = left + lw;
        if used < canvas.1 {
            out.push_str(&" ".repeat(canvas.1 - used));
        }
        out.push('\n');
    }
    out.push_str(&"\n".repeat(canvas.0.saturating_sub(h) - top));
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
    fn dims_count_visible_grid() {
        assert_eq!(ansi_dims("█\n██\n"), (2, 2));
        assert_eq!(ansi_dims("\x1b[31m███\x1b[0m\n"), (1, 3));
    }

    #[test]
    fn pad_centers_uniformly() {
        // 2 行宽 2 的图进 4×5 画布：top=1 left=1，每行总宽补齐到 5
        let out = pad_canvas("█\n██\n", (4, 5));
        let lines: Vec<&str> = out.split('\n').collect();
        assert_eq!(lines[0], "");          // 顶部空行
        assert_eq!(visible_width(lines[1]), 5);
        assert_eq!(visible_width(lines[2]), 5);
        assert_eq!(lines[3], "");          // 底部空行
        // 居中偏移一致：两行可见内容同列起始
        assert_eq!(lines[1].trim_start().len(), lines[1].len() - 1);
        assert_eq!(lines[2].trim_start().len(), lines[2].len() - 1);
    }

    #[test]
    fn pad_skips_overflow_direction() {
        // 图比画布宽：不横垫（垫了会折行破图），竖向仍居中
        let out = pad_canvas("████\n", (4, 2));
        for line in out.split('\n').filter(|l| !l.is_empty()) {
            assert_eq!(visible_width(line), 4);
        }
    }

    #[test]
    fn canvas_respects_terminal() {
        let std = CANVAS_SMALL;
        let clamped = (std.0.min(20), std.1.min(40));
        assert_eq!(clamped, (20, 40));
    }
}
