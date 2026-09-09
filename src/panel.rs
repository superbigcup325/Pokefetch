// 面板渲染：把 FetchInfo 排成 user@host 标题 + key:value 行 + 色块，
// 含窄终端下的行宽预算截值
use crate::sysinfo;

/// 面板行：user@host 标题、分隔线、蓝色 key 的 key:value、空行、两行色块。
/// budget（面板可用列数，None = 不裁）超限时截值加 …，有截断则在模块行尾
/// 追加一行暗色注释说明，防止用户把残缺值当成完整数据
pub(crate) fn panel_rows(info: &sysinfo::FetchInfo, budget: Option<usize>) -> Vec<String> {
    let mut rows = vec![
        format!(
            "\x1b[1;32m{}\x1b[0m@\x1b[1;34m{}\x1b[0m",
            info.user, info.host
        ),
        "-".repeat(info.user.chars().count() + 1 + info.host.chars().count()),
    ];
    let mut truncated = 0usize;
    for (k, v) in &info.rows {
        let mut shown = v.as_str();
        let mut cut = false;
        if let Some(budget) = budget {
            // key 加 ": " 的可见宽度，剩余给值；1 列留给 …
            let avail = budget.saturating_sub(k.chars().count() + 2);
            if v.chars().count() > avail {
                let end = v
                    .char_indices()
                    .nth(avail.saturating_sub(1))
                    .map(|(i, _)| i)
                    .unwrap_or(v.len());
                shown = &v[..end];
                cut = true;
                truncated += 1;
            }
        }
        rows.push(format!(
            "\x1b[1;34m{k}:\x1b[0m {shown}{}",
            if cut { "…" } else { "" }
        ));
    }
    if truncated > 0 {
        rows.push(format!("\x1b[2m※ {truncated} 行因终端宽度截断\x1b[0m"));
    }
    rows.push(String::new());
    // 色块与 fastfetch 同款：背景色空格条，每色 3 格无缝拼接；
    // 亮色行带 blink 属性（fastfetch 的兼容技巧），行尾 ESC[m 复位
    rows.push(format!(
        "{}\x1b[m",
        (40u8..48)
            .map(|c| format!("\x1b[{c}m   "))
            .collect::<String>()
    ));
    rows.push(format!(
        "\x1b[5m{}\x1b[m",
        (100u8..108)
            .map(|c| format!("\x1b[{c}m   "))
            .collect::<String>()
    ));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_structure() {
        let info = sysinfo::FetchInfo {
            user: "u".into(),
            host: "h".into(),
            rows: vec![("OS".into(), "x".into())],
        };
        let rows = panel_rows(&info, None);
        assert_eq!(rows.len(), 6); // title, 分隔线, kv, 空行, 色块×2
        assert!(rows[0].starts_with("\x1b[1;32mu\x1b[0m@\x1b[1;34mh"));
        assert_eq!(rows[1], "---");
        assert!(rows[2].starts_with("\x1b[1;34mOS:"));
        assert!(rows[4].starts_with("\x1b[40m   "));
        assert!(rows[4].ends_with("\x1b[m"));
        assert!(rows[5].starts_with("\x1b[5m\x1b[100m   "));
    }

    #[test]
    fn panel_budget_truncation() {
        let info = sysinfo::FetchInfo {
            user: "u".into(),
            host: "h".into(),
            rows: vec![
                ("OS".into(), "short".into()),
                ("Battery".into(), "35% [Discharging] (L21B4PC0)".into()),
            ],
        };
        // 预算 15：OS 行 "OS: short"=9 不裁；Battery 行 key+2=9，值留 6 列（5+…）
        let rows = panel_rows(&info, Some(15));
        assert!(rows[2].ends_with("short"));
        assert!(rows[3].ends_with("…"));
        assert!(!rows[3].contains("Discharging"));
        // 截断注释行插在空行之前，注明行数
        assert!(rows[4].contains("※ 1 行因终端宽度截断"));
        assert_eq!(rows.len(), 8); // title, 分隔线, kv×2, 注释, 空行, 色块×2
        // 预算极小：两个值都只剩 …（值位于复位序列之后，只能按尾部 … 断言）
        let rows = panel_rows(&info, Some(3));
        assert!(rows[2].ends_with("…") && rows[2].contains("OS:"));
        assert!(rows[3].ends_with("…") && rows[3].contains("Battery:"));
        assert!(rows[4].contains("※ 2 行"));
    }
}
