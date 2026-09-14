// CLI 表面：clap 参数定义、-r 世代区间解析、help 尾部动态段
use clap::{CommandFactory, FromArgMatches, Parser};

use crate::sysinfo;

/// 世代 → 图鉴编号区间（1-based，含端点）；names.txt 行号 = 图鉴编号；
/// gen 8 到 898 与上游对齐（899-905 洗翠新种不落入任何 -r 区间，仅 -n 可达）
const GENERATIONS: [(usize, usize); 9] = [
    (1, 151),
    (152, 251),
    (252, 386),
    (387, 493),
    (494, 649),
    (650, 721),
    (722, 809),
    (810, 898),
    (906, 1025),
];

/// "1" / "1-3" / "1,3,6" → 图鉴编号区间列表
pub(crate) fn parse_gens(spec: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    for part in spec.split(',') {
        let (a, b) = part.split_once('-').unwrap_or((part, part));
        let parsed = (a.trim().parse::<usize>(), b.trim().parse::<usize>());
        let (Ok(i), Ok(j)) = parsed else {
            crate::die(&format!("无效世代: {spec}"));
        };
        if !(1..=GENERATIONS.len()).contains(&i) || !(1..=GENERATIONS.len()).contains(&j) || i > j {
            crate::die(&format!("无效世代: {spec}"));
        }
        ranges.push((GENERATIONS[i - 1].0, GENERATIONS[j - 1].1));
    }
    ranges
}

/// 模块名清单按给定列宽贪心折行（模块名均 ASCII），供 help 展示；
/// 从 sysinfo 注册表派生，避免与 MODULES 手工双份漂移
fn module_help_lines(width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for name in sysinfo::module_names() {
        match lines.last_mut() {
            Some(l) if l.chars().count() + 1 + name.len() <= width => {
                l.push(' ');
                l.push_str(name);
            }
            _ => lines.push(name.to_string()),
        }
    }
    lines
}

/// 帮助尾部的动态段：fastfetch 对接 + 模块清单（注册表派生）+ 适配说明
fn help_tail() -> String {
    let mod_lines = module_help_lines(72)
        .iter()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "fastfetch 对接:
  --raw          只输出字符画本体（无名字行无面板）:
                 fastfetch --data-raw \"$(pokefetch -r --raw)\"
  -o, --output   字符画写入文件（stdout 不输出）
  --logo-cache   写入 ~/.cache/pokefetch/logo.ans 并照常打印；
                 配合仓库附带的 fastfetch.jsonc 使用（fastfetch --config）

面板模块 (--modules 可选):
{mod_lines}

随机时自动跳过当前终端放不下的精灵；显式 -n/-b 不做干预、原样输出。
面板信息来自 /proc、/sys 与环境变量，取不到的行自动跳过。"
    )
}

#[derive(Parser)]
#[command(
    name = "pokefetch",
    about = "终端里的宝可梦 fetch：精灵字符画 + 系统信息面板"
)]
pub(crate) struct Args {
    /// 指定宝可梦（pikachu；形态配 -f，或直接传全名 charizard-mega-x）
    #[arg(short, long)]
    pub(crate) name: Option<String>,

    /// 随机一只，可附加世代: 1 / 1-3 / 1,3,6
    #[arg(short, long, num_args(0..=1), default_missing_value = "")]
    pub(crate) random: Option<String>,

    /// 从逗号分隔的名字列表中随机（如 pikachu,gengar）
    #[arg(
        long,
        value_name = "列表",
        conflicts_with_all = ["name", "random"]
    )]
    pub(crate) random_by_names: Option<String>,

    /// 指定形态（-l 查名字，形态见 assets/pokemon.json，如 mega-x；需配 -n）
    #[arg(short = 'f', long, requires = "name")]
    pub(crate) form: Option<String>,

    /// 强制闪光版（不带时随机模式有 1/128 概率出 shiny；-n 指定不掷点）
    #[arg(short, long)]
    pub(crate) shiny: bool,

    /// 大尺寸字符画（默认 small）
    #[arg(short, long)]
    pub(crate) big: bool,

    /// 画布宽度：左锚右垫到此列宽（0=关闭；默认 small 40、large 不垫）
    #[arg(long, value_name = "列宽")]
    pub(crate) canvas: Option<usize>,

    /// 精灵在画布内居中（默认左锚）
    #[arg(long)]
    pub(crate) center: bool,

    /// 只输出精灵，不带系统信息面板
    #[arg(long)]
    pub(crate) no_panel: bool,

    /// 面板模块选择，逗号分隔（默认为精选集；未知模块报错）
    #[arg(long, value_name = "列表")]
    pub(crate) modules: Option<String>,

    /// 显示精灵名字行（面板模式下默认不显示）
    #[arg(long, conflicts_with = "no_title")]
    pub(crate) title: bool,

    /// 不显示名字行（纯精灵模式下默认显示）
    #[arg(long)]
    pub(crate) no_title: bool,

    /// 只输出字符画本体（无名字行无面板），作 fastfetch logo 源
    #[arg(long)]
    pub(crate) raw: bool,

    /// 字符画写入文件（stdout 不输出）
    #[arg(short, long, value_name = "文件")]
    pub(crate) output: Option<std::path::PathBuf>,

    /// 写入 ~/.cache/pokefetch/logo.ans 并照常打印
    #[arg(long)]
    pub(crate) logo_cache: bool,

    /// 动画播放（需独立数据文件 anim.bin；仅 stdout 直连终端时生效，
    /// 数据缺失或非 tty 回退静态图；与 --raw/-o/--logo-cache 互斥）
    #[arg(
        long,
        conflicts_with_all = ["raw", "output", "logo_cache"]
    )]
    pub(crate) animated: bool,

    /// 动画固定播放轮数后退出（省缺=无限循环，按 q 退出）
    #[arg(long, value_name = "轮数", requires = "animated")]
    pub(crate) loops: Option<usize>,

    /// 常驻重绘：终端尺寸变化（含 niri 等合成器全屏/平铺切换）即整帧重排；
    /// q/Esc/Ctrl-C 退出，r 重掷一只。需交互终端（与 --raw/-o/--logo-cache/--animated 互斥）
    #[arg(
        long,
        conflicts_with_all = ["raw", "output", "logo_cache", "animated"]
    )]
    pub(crate) watch: bool,

    /// 维护命令：把帧目录打包为 anim.bin（-o 指定输出路径）
    #[arg(long, value_name = "帧目录", hide = true)]
    pub(crate) anim_pack: Option<std::path::PathBuf>,

    /// 列出全部名字
    #[arg(short, long)]
    pub(crate) list: bool,
}

/// 解析命令行（动态 after_help 注入注册表派生的模块清单）
pub(crate) fn parse_args() -> Args {
    let matches = Args::command().after_help(help_tail()).get_matches();
    Args::from_arg_matches(&matches).unwrap()
}
