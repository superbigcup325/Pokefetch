// 画布与排版几何：可见宽度测量、左锚/居中垫宽、精灵与面板逐行拼接

/// 行的可见宽度（字形均单宽；跳过 \x1b[..m 转义段）
pub(crate) fn visible_width(line: &str) -> usize {
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

/// 全图最大可见行宽（调用方与 pad_canvas 共享，整个输出管线只扫一次）
pub(crate) fn max_visible_width(ansi: &str) -> usize {
    ansi.lines().map(visible_width).max().unwrap_or(0)
}

/// 单字符终端格宽。素材字形（空格/▀/▄/█）与 ASCII 均 1 格；
/// 面板侧会出现 CJK 文本与 ✨，实占 2 格，按 1 格算会低估行宽导致折行。
/// 封闭集哲学：显式收录宽区（CJK 统一表意/假名/谚文/全角形式/扩展区/宽 emoji），
/// 未收录字符一律按 1 格兜底——宁可极端字符偶见溢出，不做运行时查表
pub(crate) fn char_cells(ch: char) -> usize {
    match ch as u32 {
        0x1100..=0x11FF // 谚文字母（Jamo）
        | 0x2E80..=0x9FFF // CJK 部首/假名/注音/兼容标点/统一表意
        | 0xAC00..=0xD7AF // 谚文音节
        | 0xF900..=0xFAFF // 兼容表意
        | 0xFE30..=0xFE4F // CJK 兼容形式
        | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 // 全角形式
        | 0x1F300..=0x1FAFF // emoji（默认 emoji presentation，宽）
        | 0x20000..=0x3134F // CJK 扩展 B-F
        | 0x2728 => 2, // ✨ sparkles（dingbats 区例外，本项目唯一自产 emoji）
        _ => 1,
    }
}

/// 字符串占格数：与 visible_width 同构，差别仅在宽字符按 char_cells 计 2 格
pub(crate) fn str_cells(s: &str) -> usize {
    let mut w = 0;
    let mut esc = false;
    for ch in s.chars() {
        if esc {
            if ch == 'm' {
                esc = false;
            }
        } else if ch == '\x1b' {
            esc = true;
        } else {
            w += char_cells(ch);
        }
    }
    w
}

/// 画布：精灵整体在画布内左锚或居中，每行右垫空格到画布宽
/// （fastfetch 面板列位由此稳定）；行宽已达画布的行不动（精灵超宽时自然伸出，永不裁剪）。
/// 居中按精灵整体最大宽计算统一左偏移，逐行对齐不被打散；
/// max_w 由调用方传入（即 max_visible_width 的结果），避免重复扫描
pub(crate) fn pad_canvas(ansi: &str, w: usize, center: bool, max_w: usize) -> String {
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

/// 画布宽度决策：显式 --canvas 原样生效（0=关闭，两尺寸都垫、不随终端收窄）；
/// 默认 small=DEFAULT_CANVAS 并随终端收窄，large 不垫（-b 是刻意行为）。
/// main 一次性输出与 --watch 逐帧重绘共用，保证口径唯一
pub(crate) fn resolve_canvas(
    explicit: Option<usize>,
    big: bool,
    term_cols: Option<usize>,
) -> usize {
    match explicit {
        Some(0) => 0,
        Some(n) => n,
        None => {
            if big {
                0
            } else {
                match term_cols {
                    Some(cols) => DEFAULT_CANVAS.min(cols),
                    None => DEFAULT_CANVAS,
                }
            }
        }
    }
}

/// 默认画布宽（列）：small 输出左锚、右垫到该宽度，fastfetch 面板列位由此稳定
const DEFAULT_CANVAS: usize = 40;

/// 精灵与面板逐行拼接：精灵侧统一垫到 sprite_w，矮的一侧自然延续到末尾
pub(crate) fn compose(sprite: &str, panel: Vec<String>, sprite_w: usize, gap: usize) -> String {
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
        assert_eq!(pad_canvas("█\n██\n", 4, false, 2), "█   \n██  \n");
    }

    #[test]
    fn pad_centers_with_uniform_offset() {
        // 精灵最大宽 2，画布 5：统一左偏移 1，逐行右垫到 5
        assert_eq!(pad_canvas("█\n██\n", 5, true, 2), " █   \n ██  \n");
    }

    #[test]
    fn pad_skips_wide_lines() {
        // 行宽已达画布：不垫（精灵超宽自然伸出）
        assert_eq!(pad_canvas("████\n", 2, false, 4), "████\n");
        assert_eq!(pad_canvas("████\n", 2, true, 4), "████\n");
    }

    #[test]
    fn pad_zero_is_noop() {
        assert_eq!(pad_canvas("█\n", 0, false, 1), "█\n");
        assert_eq!(pad_canvas("█\n", 0, true, 1), "█\n");
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
    fn cells_count_wide_chars_double() {
        // 素材字形与 ASCII 1 格；CJK/全角/✨ 2 格；转义段不计
        assert_eq!(str_cells("▀▄█ "), 4);
        assert_eq!(str_cells("行宽截断"), 8);
        assert_eq!(str_cells("ＡＢ"), 4); // 全角拉丁
        assert_eq!(str_cells("✨"), 2);
        assert_eq!(str_cells("※…"), 2); // 区分于宽字符的窄标点
        assert_eq!(str_cells("\x1b[38;2;1;2;3m行\x1b[0m"), 2);
    }

    #[test]
    fn canvas_resolution_rules() {
        // 显式值原样生效（0 关闭），默认随终端收窄，large 不垫
        assert_eq!(resolve_canvas(Some(0), false, Some(80)), 0);
        assert_eq!(resolve_canvas(Some(56), true, Some(80)), 56);
        assert_eq!(resolve_canvas(None, false, Some(80)), 40);
        assert_eq!(resolve_canvas(None, false, Some(30)), 30);
        assert_eq!(resolve_canvas(None, false, None), 40);
        assert_eq!(resolve_canvas(None, true, Some(80)), 0);
    }
}
