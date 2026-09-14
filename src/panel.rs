// 面板渲染：把 FetchInfo 排成 user@host 标题 + 分隔线 + Dex 行 + key:value 行
// + 截断注释 + 空行 + 两行色块。
//
// 窄终端防折行的关键是"所有行都受预算约束"：此前只有 key:value 的值被截，
// 标题/分隔线/色块（固定 24 格）/Dex 行/注释行原样输出，中窄终端必溢出换行。
// 现在每一行（含固定行）统一按 budget 截断；色块按预算降级（每色格数 3→2→1
// 再减色数）；宽字符按 char_cells 计 2 格，截断位置按格宽而非字符数。
// budget = None 表示不裁（无终端检测或显式 --modules 硬打）。
use crate::canvas::{char_cells, str_cells};
use crate::sysinfo;

/// 行宽预算：默认模块集 + 有终端检测时 = 终端宽 - 精灵区宽 - gap；
/// 显式 --modules 硬打不裁（对齐 -b 硬打哲学）返回 None；
/// 返回 Some(0) 表示终端只够精灵区——gap 与省略号残行也放不下，
/// 调用方应让面板整体让位（不出面板），而非按 0 格截断
pub(crate) fn row_budget(
    default_modules: bool,
    term_cols: Option<usize>,
    sprite_w: usize,
) -> Option<usize> {
    if !default_modules {
        return None;
    }
    term_cols.map(|cols| cols.saturating_sub(sprite_w + 3))
}

/// 整行截断到 max 格：转义段原样通过不计宽；截断处加 …（预留 1 格）并补
/// 复位序列（截断点之后未复制的 SGR 尾部不能悬空）。返回 (截后行, 是否截断)
fn truncate_cells(s: &str, max: usize) -> (String, bool) {
    if str_cells(s) <= max {
        return (s.to_string(), false);
    }
    if max == 0 {
        return (String::new(), true);
    }
    let limit = max - 1; // … 占 1 格
    let mut out = String::with_capacity(s.len() + 8);
    let mut esc = false;
    let mut w = 0usize;
    for ch in s.chars() {
        if esc {
            out.push(ch);
            if ch == 'm' {
                esc = false;
            }
            continue;
        }
        if ch == '\x1b' {
            esc = true;
            out.push(ch);
            continue;
        }
        let cw = char_cells(ch);
        if w + cw > limit {
            break;
        }
        w += cw;
        out.push(ch);
    }
    out.push('…');
    out.push_str("\x1b[0m");
    (out, true)
}

/// 预算内套截断，截断计数累加到 *truncated（注释行不参与计数）
fn fit(row: String, budget: Option<usize>, truncated: &mut usize) -> String {
    match budget {
        Some(b) => {
            let (r, cut) = truncate_cells(&row, b);
            if cut {
                *truncated += 1;
            }
            r
        }
        None => row,
    }
}

/// 色块降级尺寸：每色格数 = clamp(预算/8, 1, 3)，色数 = min(8, 预算/格数)；
/// 无预算（不裁）= 满配 8 色 × 3 格。预算为 0 色数为 0，整行省略
fn color_dims(budget: Option<usize>) -> (usize, usize) {
    let b = budget.unwrap_or(24);
    let per = (b / 8).clamp(1, 3);
    let count = (b / per).min(8);
    (count, per)
}

/// 色块行：背景色空格条，与 fastfetch 同款无缝拼接；
/// bright 行带 blink 属性（fastfetch 的兼容技巧）
fn color_row(base: u8, count: usize, per: usize, blink: bool) -> String {
    if count == 0 {
        return String::new();
    }
    let blocks: String = (base..base + count as u8)
        .map(|c| format!("\x1b[{c}m{}", " ".repeat(per)))
        .collect();
    if blink {
        format!("\x1b[5m{blocks}\x1b[m")
    } else {
        format!("{blocks}\x1b[m")
    }
}

/// 面板行：标题、分隔线、Dex 行（可选，插在分隔线后）、key:value 行、
/// 截断注释（有截断才出现）、空行、两行色块。所有行都保证 ≤ budget（不裁时除外）
pub(crate) fn panel_rows(
    info: &sysinfo::FetchInfo,
    budget: Option<usize>,
    dex: Option<String>,
) -> Vec<String> {
    let mut truncated = 0usize;
    let title = fit(
        format!(
            "\x1b[1;32m{}\x1b[0m@\x1b[1;34m{}\x1b[0m",
            info.user, info.host
        ),
        budget,
        &mut truncated,
    );
    // 分隔线与（截后）标题同宽，保持对齐
    let mut rows = vec![title.clone(), "-".repeat(str_cells(&title))];
    if let Some(d) = dex {
        rows.push(fit(d, budget, &mut truncated));
    }
    for (k, v) in &info.rows {
        rows.push(fit(
            format!("\x1b[1;34m{k}:\x1b[0m {v}"),
            budget,
            &mut truncated,
        ));
    }
    if truncated > 0 {
        let note = format!("\x1b[2m※ {truncated} 行因终端宽度截断\x1b[0m");
        rows.push(truncate_cells(&note, budget.unwrap_or(usize::MAX)).0);
    }
    rows.push(String::new());
    let (count, per) = color_dims(budget);
    rows.push(color_row(40, count, per, false));
    rows.push(color_row(100, count, per, true));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(rows: Vec<(&str, &str)>) -> sysinfo::FetchInfo {
        sysinfo::FetchInfo {
            user: "u".into(),
            host: "h".into(),
            rows: rows
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn budget_rule() {
        // 仅默认模块集且有终端检测时才有预算；Some(0) 表示只够精灵区
        assert_eq!(row_budget(false, Some(80), 40), None);
        assert_eq!(row_budget(true, None, 40), None);
        assert_eq!(row_budget(true, Some(80), 40), Some(37));
        assert_eq!(row_budget(true, Some(43), 40), Some(0));
        assert_eq!(row_budget(true, Some(42), 40), Some(0));
    }

    #[test]
    fn panel_structure() {
        let rows = panel_rows(&info(vec![("OS", "x")]), None, None);
        assert_eq!(rows.len(), 6); // 标题, 分隔线, kv, 空行, 色块×2
        assert!(rows[0].starts_with("\x1b[1;32mu\x1b[0m@\x1b[1;34mh"));
        assert_eq!(rows[1], "---");
        assert!(rows[2].starts_with("\x1b[1;34mOS:"));
        assert!(rows[4].starts_with("\x1b[40m   "));
        assert!(rows[4].ends_with("\x1b[m"));
        assert!(rows[5].starts_with("\x1b[5m\x1b[100m   "));
        // 不裁时色块满配 8 色 × 3 格 = 24 格
        assert_eq!(str_cells(&rows[4]), 24);
    }

    #[test]
    fn dex_row_after_separator() {
        let rows = panel_rows(
            &info(vec![("OS", "x")]),
            None,
            Some("\x1b[1;34mDex:\x1b[0m #025".into()),
        );
        assert!(rows[2].contains("#025"));
        assert!(rows[3].starts_with("\x1b[1;34mOS:"));
    }

    #[test]
    fn budget_truncates_every_row() {
        let long = sysinfo::FetchInfo {
            user: "u".into(),
            host: "verylonghostname".into(), // 标题 18 格，超预算
            rows: vec![
                ("OS".into(), "short".into()),
                ("Battery".into(), "35% [Discharging]".into()),
            ],
        };
        let rows = panel_rows(
            &long,
            Some(15),
            Some("\x1b[1;34mDex:\x1b[0m #025 ✨".into()),
        );
        // 短行原样
        assert!(rows[3].ends_with("short"));
        // 超宽的标题截到预算内并带 …（尾部补复位序列），分隔线与截后标题同宽
        assert_eq!(str_cells(&rows[0]), 15);
        assert!(rows[0].contains('…'));
        assert!(rows[0].ends_with("\x1b[0m"));
        assert_eq!(rows[1], "-".repeat(15));
        // Dex 行 12 格（✨ 按 2 格）在预算内，原样保留
        assert_eq!(str_cells(&rows[2]), 12);
        assert!(rows[2].ends_with("✨"));
        // 超宽的值行截到预算内
        assert_eq!(str_cells(&rows[4]), 15);
        assert!(rows[4].contains('…'));
        // 截断注释行自身也在预算内（预算 15 只够显示前半句，含截断计数）
        assert!(rows[5].starts_with("\x1b[2m※"));
        assert!(rows[5].contains("行因终端宽"));
        assert!(str_cells(&rows[5]) <= 15);
        // 色块降级：预算 15 → 每色 1 格 × 8 色（索引 6 为空行）
        assert_eq!(str_cells(&rows[7]), 8);
    }

    #[test]
    fn color_degradation() {
        assert_eq!(color_dims(None), (8, 3));
        assert_eq!(color_dims(Some(24)), (8, 3));
        assert_eq!(color_dims(Some(23)), (8, 2));
        assert_eq!(color_dims(Some(16)), (8, 2));
        assert_eq!(color_dims(Some(15)), (8, 1));
        assert_eq!(color_dims(Some(5)), (5, 1));
        assert_eq!(color_dims(Some(0)), (0, 1));
    }

    #[test]
    fn wide_char_values_count_double() {
        // 宽字符按 2 格计量："K: " 3 格 + 预算 6 → 只放得下 1 汉字 + …
        let rows = panel_rows(&info(vec![("K", "汉字测试")]), Some(6), None);
        assert_eq!(str_cells(&rows[2]), 6);
        assert!(rows[2].contains("汉"));
        assert!(!rows[2].contains("测试"));
    }

    #[test]
    fn degenerate_tiny_budget() {
        // 预算 3：标题 "u@h" 恰好 3 格原样，两个值行截断（计数 2）
        let rows = panel_rows(
            &info(vec![("OS", "short"), ("Battery", "35%")]),
            Some(3),
            None,
        );
        for (i, r) in rows.iter().enumerate() {
            assert!(str_cells(r) <= 3, "行 {i} 超预算: {r:?}");
        }
        // 预算 3 连注释行都放不下，只剩 ※ 和 …（无 dex 行，注释在索引 4）
        assert!(rows[4].starts_with("\x1b[2m※"));
    }
}
