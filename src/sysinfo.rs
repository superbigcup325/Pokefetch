// 系统信息数据层：读 /proc、/sys 与环境变量，纯 std 零依赖。
// 信息缺失一律返回 None（对应行被跳过），不引入 panic 路径。

/// 面板一行 key: value 的纯数据
pub type Row = (String, String);

pub struct FetchInfo {
    pub user: String,
    pub host: String,
    pub rows: Vec<Row>,
}

pub fn collect() -> FetchInfo {
    FetchInfo {
        user: username(),
        host: hostname(),
        rows: collect_rows(),
    }
}

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

fn username() -> String {
    std::env::var("USER").unwrap_or_else(|_| "unknown".into())
}

fn hostname() -> String {
    read("/proc/sys/kernel/hostname").unwrap_or_else(|| "unknown".into())
}

fn collect_rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let mut push = |k: &str, v: Option<String>| {
        if let Some(v) = v {
            rows.push((k.to_string(), v));
        }
    };
    push("OS", os());
    push("Host", host());
    push("Kernel", kernel());
    push("Uptime", uptime());
    push("Packages", packages());
    push("Shell", shell());
    push("DE", de());
    push("Terminal", terminal());
    push("CPU", cpu());
    push("Memory", memory());
    rows
}

fn os() -> Option<String> {
    let pretty = parse_pretty_os(&read("/etc/os-release")?)?;
    Some(format!("{pretty} {}", std::env::consts::ARCH))
}

fn parse_pretty_os(content: &str) -> Option<String> {
    content
        .lines()
        .find(|l| l.starts_with("PRETTY_NAME="))
        .map(|l| l["PRETTY_NAME=".len()..].trim_matches('"').to_string())
}

fn host() -> Option<String> {
    const DMI: &str = "/sys/devices/virtual/dmi/id";
    read(&format!("{DMI}/product_name"))
        .filter(|s| !s.is_empty())
        .or_else(|| read(&format!("{DMI}/board_name")).filter(|s| !s.is_empty()))
}

fn kernel() -> Option<String> {
    read("/proc/sys/kernel/osrelease")
}

fn uptime() -> Option<String> {
    let t = read("/proc/uptime")?;
    let secs = t.split_whitespace().next()?.parse::<u64>().ok()?;
    Some(fmt_uptime(secs))
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

fn packages() -> Option<String> {
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

fn shell() -> Option<String> {
    let s = std::env::var("SHELL").ok()?;
    let base = s.rsplit('/').next().unwrap_or(&s);
    (!base.is_empty()).then(|| base.to_string())
}

fn de() -> Option<String> {
    let d = std::env::var("XDG_CURRENT_DESKTOP").ok()?;
    d.split(':').find(|s| !s.is_empty()).map(str::to_string)
}

/// 沿 /proc ppid 链上溯，跳过 shell 进程，第一个非 shell 即终端
fn terminal() -> Option<String> {
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

fn cpu() -> Option<String> {
    let info = read("/proc/cpuinfo")?;
    let model = info
        .lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())?;
    let cores = info.lines().filter(|l| l.starts_with("processor")).count();
    Some(format!("{model} ({cores})"))
}

fn memory() -> Option<String> {
    let info = read("/proc/meminfo")?;
    let total = meminfo_kib(&info, "MemTotal")?;
    let avail = meminfo_kib(&info, "MemAvailable")?;
    Some(fmt_memory(total, avail))
}

fn meminfo_kib(content: &str, field: &str) -> Option<u64> {
    content
        .lines()
        .find(|l| l.starts_with(field))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
}

/// 已用 / 总量（GiB）+ 百分比
fn fmt_memory(total_kib: u64, avail_kib: u64) -> String {
    let used = total_kib.saturating_sub(avail_kib);
    let gib = |kib: u64| kib as f64 / (1 << 20) as f64;
    let pct = if total_kib > 0 {
        used as f64 / total_kib as f64 * 100.0
    } else {
        0.0
    };
    format!(
        "{:.2} GiB / {:.2} GiB ({:.0}%)",
        gib(used),
        gib(total_kib),
        pct
    )
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
    fn os_release_parsing() {
        assert_eq!(
            parse_pretty_os("NAME=\"Arch\"\nPRETTY_NAME=\"Arch Linux\"\n"),
            Some("Arch Linux".into())
        );
        assert_eq!(parse_pretty_os("ID=arch\n"), None);
    }

    #[test]
    fn memory_format() {
        // 16 GiB 总量、约 8.44 GiB 已用
        assert_eq!(fmt_memory(16777216, 7931782), "8.44 GiB / 16.00 GiB (53%)");
    }
}
