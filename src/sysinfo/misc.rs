// 系统杂项模块：OS/主机/内核/运行时长/包数/Shell/终端/负载/地区，
// 数据来自 /proc、/sys dmi、pacman 包库与环境变量
use super::read;

pub(super) fn os() -> Option<String> {
    let pretty = parse_pretty_os(&read("/etc/os-release")?)?;
    Some(format!("{pretty} {}", std::env::consts::ARCH))
}

fn parse_pretty_os(content: &str) -> Option<String> {
    content
        .lines()
        .find(|l| l.starts_with("PRETTY_NAME="))
        .map(|l| l["PRETTY_NAME=".len()..].trim_matches('"').to_string())
}

pub(super) fn host() -> Option<String> {
    let name = dmi("product_name")
        .filter(|s| !s.is_empty())
        .or_else(|| dmi("board_name").filter(|s| !s.is_empty()))?;
    // 联想等厂商把营销名放 product_family，拼成 "82RC (Legion Y7000P IAH7)"
    match dmi("product_family").filter(|s| !s.is_empty() && *s != name) {
        Some(family) => Some(format!("{name} ({family})")),
        None => Some(name),
    }
}

pub(super) fn board() -> Option<String> {
    let vendor = dmi("board_vendor")?;
    let name = dmi("board_name")?;
    Some(format!("{vendor} {name}"))
}

pub(super) fn bios() -> Option<String> {
    let vendor = dmi("bios_vendor")?;
    let version = dmi("bios_version").unwrap_or_default();
    let date = dmi("bios_date").unwrap_or_default();
    let mut s = vendor;
    if !version.is_empty() {
        s.push(' ');
        s.push_str(&version);
    }
    if !date.is_empty() {
        s.push_str(&format!(" ({date})"));
    }
    Some(s)
}

fn dmi(file: &str) -> Option<String> {
    read(&format!("/sys/devices/virtual/dmi/id/{file}"))
}

pub(super) fn kernel() -> Option<String> {
    let release = read("/proc/sys/kernel/osrelease")?;
    let os_type = read("/proc/sys/kernel/ostype").unwrap_or_else(|| std::env::consts::OS.into());
    Some(format!("{os_type} {release}"))
}

pub(super) fn uptime() -> Option<String> {
    let t = read("/proc/uptime")?;
    Some(fmt_uptime(uptime_secs(&t)?))
}

/// /proc/uptime 首字段是浮点秒（如 "9623.03"），取整
fn uptime_secs(t: &str) -> Option<u64> {
    let s = t.split_whitespace().next()?.parse::<f64>().ok()?;
    Some(s.max(0.0) as u64)
}

/// 秒 → "2 hours, 51 mins" 风格：取最长的两段非零单位
fn fmt_uptime(secs: u64) -> String {
    const MIN: u64 = 60;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;
    let units = [
        (secs / DAY, "day"),
        (secs % DAY / HOUR, "hour"),
        (secs % HOUR / MIN, "min"),
    ];
    let nonzero: Vec<String> = units
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, u)| format!("{n} {u}{}", if *n > 1 { "s" } else { "" }))
        .collect();
    match nonzero.len() {
        0 => "0 mins".into(),
        _ => nonzero.into_iter().take(2).collect::<Vec<_>>().join(", "),
    }
}

pub(super) fn packages() -> Option<String> {
    if let Some(n) = std::fs::read_dir("/var/lib/pacman/local")
        .ok()
        .map(|d| d.count())
        .filter(|&n| n > 0)
    {
        return Some(format!("{n} (pacman)"));
    }
    if let Ok(status) = std::fs::read_to_string("/var/lib/dpkg/status") {
        let n = status.lines().filter(|l| l.starts_with("Package:")).count();
        if n > 0 {
            return Some(format!("{n} (dpkg)"));
        }
    }
    None
}

pub(super) fn shell() -> Option<String> {
    let s = std::env::var("SHELL").ok()?;
    let base = s.rsplit('/').next().unwrap_or(&s);
    if base.is_empty() {
        return None;
    }
    // 版本走 pacman 包库（可执行名≈包名），非 Arch 或查不到时只显示名字
    match pacman_version(base) {
        Some(v) => Some(format!("{base} {v}")),
        None => Some(base.to_string()),
    }
}

/// pacman 本地包库 %VERSION%（剥掉 pkgrel）；shell 与 desktop 的 DE 版本共用
pub(super) fn pacman_version(pkg: &str) -> Option<String> {
    // 目录名形如 pkg-version-rel，按前缀匹配
    let dir = std::fs::read_dir("/var/lib/pacman/local").ok()?;
    let found = dir.flatten().map(|e| e.path()).find(|p| {
        p.file_name().is_some_and(|n| {
            let n = n.to_string_lossy();
            n == pkg || n.starts_with(&format!("{pkg}-"))
        })
    })?;
    pacman_version_inner(&read(&format!("{}/desc", found.display()))?)
}

fn pacman_version_inner(desc: &str) -> Option<String> {
    let mut section = "";
    for line in desc.lines() {
        if line.starts_with('%') && line.ends_with('%') {
            section = &line[1..line.len() - 1];
            continue;
        }
        if section == "VERSION" && !line.is_empty() {
            return Some(line.split('-').next().unwrap_or(line).to_string());
        }
    }
    None
}

/// 沿 /proc ppid 链上溯，跳过 shell 进程，第一个非 shell 即终端
pub(super) fn terminal() -> Option<String> {
    const SHELLS: [&str; 8] = ["zsh", "bash", "fish", "sh", "dash", "ksh", "nu", "pwsh"];
    let mut pid = std::process::id();
    for _ in 0..12 {
        let ppid = ppid_of(pid)?;
        if ppid <= 1 {
            return None;
        }
        let comm = read(&format!("/proc/{ppid}/comm"))?;
        if !SHELLS.contains(&comm.as_str()) {
            return Some(comm);
        }
        pid = ppid;
    }
    None
}

/// /proc/<pid>/stat 第 4 字段 ppid（comm 可含空格，取最后一个 ')' 之后）
fn ppid_of(pid: u32) -> Option<u32> {
    let stat = read(&format!("/proc/{pid}/stat"))?;
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

pub(super) fn load() -> Option<String> {
    let t = read("/proc/loadavg")?;
    let parts: Vec<&str> = t.split_whitespace().collect();
    Some(format!(
        "{} {} {}",
        parts.first()?,
        parts.get(1)?,
        parts.get(2)?
    ))
}

pub(super) fn locale() -> Option<String> {
    ["LC_ALL", "LANG"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_units() {
        assert_eq!(fmt_uptime(0), "0 mins");
        assert_eq!(fmt_uptime(3540), "59 mins");
        assert_eq!(fmt_uptime(1710), "28 mins");
        assert_eq!(fmt_uptime(3600), "1 hour");
        assert_eq!(fmt_uptime(5460), "1 hour, 31 mins");
        assert_eq!(fmt_uptime(90000), "1 day, 1 hour");
    }

    #[test]
    fn uptime_parses_fractional() {
        assert_eq!(uptime_secs("9623.03 148590.88"), Some(9623));
        assert_eq!(uptime_secs("0.5 0"), Some(0));
        assert_eq!(uptime_secs("abc"), None);
    }

    #[test]
    fn pacman_version_parsing() {
        let desc = "%NAME%\nplasma-workspace\n\n%VERSION%\n6.7.4-1\n\n%BASE%\nx\n";
        assert_eq!(pacman_version_inner(desc), Some("6.7.4".into()));
    }

    #[test]
    fn os_release_parsing() {
        assert_eq!(
            parse_pretty_os("NAME=\"Arch\"\nPRETTY_NAME=\"Arch Linux\"\n"),
            Some("Arch Linux".into())
        );
        assert_eq!(parse_pretty_os("ID=arch\n"), None);
    }
}
