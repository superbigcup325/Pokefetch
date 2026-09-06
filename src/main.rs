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
  -o, --output <文件>  字符画写入文件（stdout 不输出）
      --logo-cache     写入 ~/.cache/pokefetch/logo.ans 并照常打印；
                       配合仓库附带的 fastfetch.jsonc 使用（fastfetch --config）
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
    let ansi = sprite_ansi(&index, &chosen, shiny, big);

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
