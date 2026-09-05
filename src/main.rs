// pokefetch —— 随机/指定打印宝可梦字符画
// 素材编译期内嵌：build.rs 扫描 assets/ 生成 static SPRITES（key = "size/variant/name"）
include!(concat!(env!("OUT_DIR"), "/sprites_gen.rs"));

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

fn print_pokemon(name: &str, shiny: bool, big: bool, title: bool) {
    let size = if big { "large" } else { "small" };
    let variant = if shiny { "shiny" } else { "regular" };
    let key = format!("{size}/{variant}/{name}");
    match SPRITES.binary_search_by(|(k, _)| (*k).cmp(key.as_str())) {
        Ok(i) => {
            if title {
                println!("{name}{}", if shiny { " (shiny)" } else { "" });
            }
            print!("{}", SPRITES[i].1);
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

    print_pokemon(&chosen, shiny, big, title);
}
