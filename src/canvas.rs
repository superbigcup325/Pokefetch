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
}
