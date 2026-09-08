// 系统信息数据层：读 /proc、/sys 与环境变量，纯 std 零依赖。
// 信息缺失一律返回 None（对应行被跳过），不引入 panic 路径。
//
// 结构：MODULES 注册表是单一事实源——模块名、取值函数、展开顺序、
// 默认集与 --modules 校验全部由它派生；collect 只按解析出的名单展开。

/// 面板一行 key: value 的纯数据
pub type Row = (String, String);

pub struct FetchInfo {
    pub user: String,
    pub host: String,
    pub rows: Vec<Row>,
}

/// 模块取值函数签名
type Fetcher = fn() -> Option<String>;

/// 模块注册表：模块名（--modules 用）→ 显示标签 → 取值函数
const MODULES: &[(&str, &str, Fetcher)] = &[
    ("os", "OS", os),
    ("host", "Host", host),
    ("board", "Board", board),
    ("bios", "BIOS", bios),
    ("kernel", "Kernel", kernel),
    ("uptime", "Uptime", uptime),
    ("packages", "Packages", packages),
    ("shell", "Shell", shell),
    ("de", "DE", de),
    ("wm", "WM", wm),
    ("terminal", "Terminal", terminal),
    ("gpu", "GPU", gpu),
    ("cpu", "CPU", cpu),
    ("memory", "Memory", memory),
    ("swap", "Swap", swap),
    ("disk", "Disk", disk),
    ("battery", "Battery", battery),
    ("load", "Load", load),
    ("locale", "Locale", locale),
];

/// 默认集（面板精选；board/bios 偏冷门，经 --modules 点名启用）
const DEFAULT_SELECTED: &[&str] = &[
    "os", "host", "kernel", "uptime", "packages", "shell", "de", "wm", "terminal", "gpu", "cpu",
    "memory", "swap", "disk", "battery", "load", "locale",
];

/// 全部可用模块名（帮助/报错用）
pub fn module_names() -> impl Iterator<Item = &'static str> {
    MODULES.iter().map(|(n, _, _)| *n)
}

/// 把用户选择解析为按注册表顺序的名单（选择序无关、自动去重）；
/// 含未知模块名时报错
pub fn resolve(selected: Option<&[String]>) -> Result<Vec<&'static str>, String> {
    let chosen: Vec<&str> = match selected {
        None => DEFAULT_SELECTED.to_vec(),
        Some(list) => {
            let list: Vec<&str> = list.iter().map(String::as_str).collect();
            for name in &list {
                if !MODULES.iter().any(|(n, _, _)| n == name) {
                    return Err(format!(
                        "未知模块: {name}（可用: {}）",
                        module_names().collect::<Vec<_>>().join(" ")
                    ));
                }
            }
            list
        }
    };
    Ok(MODULES
        .iter()
        .map(|(n, _, _)| *n)
        .filter(|n| chosen.contains(n))
        .collect())
}

pub fn collect(selected: &[&str]) -> FetchInfo {
    FetchInfo {
        user: username(),
        host: hostname(),
        rows: MODULES
            .iter()
            .filter(|(n, _, _)| selected.contains(n))
            .filter_map(|(_, label, f)| f().map(|v| ((*label).to_string(), v)))
            .collect(),
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

// ---- 各模块：每个函数自含数据来源与格式化 ----

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
    let name = dmi("product_name")
        .filter(|s| !s.is_empty())
        .or_else(|| dmi("board_name").filter(|s| !s.is_empty()))?;
    // 联想等厂商把营销名放 product_family，拼成 "82RC (Legion Y7000P IAH7)"
    match dmi("product_family").filter(|s| !s.is_empty() && *s != name) {
        Some(family) => Some(format!("{name} ({family})")),
        None => Some(name),
    }
}

fn board() -> Option<String> {
    let vendor = dmi("board_vendor")?;
    let name = dmi("board_name")?;
    Some(format!("{vendor} {name}"))
}

fn bios() -> Option<String> {
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

fn kernel() -> Option<String> {
    let release = read("/proc/sys/kernel/osrelease")?;
    let os_type = read("/proc/sys/kernel/ostype").unwrap_or_else(|| std::env::consts::OS.into());
    Some(format!("{os_type} {release}"))
}

fn uptime() -> Option<String> {
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

fn wm() -> Option<String> {
    let de = std::env::var("XDG_CURRENT_DESKTOP").ok()?;
    let name = wm_name(de.split(':').find(|s| !s.is_empty())?)?;
    match std::env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("wayland") => Some(format!("{name} (Wayland)")),
        Ok("x11") => Some(format!("{name} (X11)")),
        _ => Some(name.into()),
    }
}

/// DE → WM 已知映射（桌面环境启动的合成器是确定的）
fn wm_name(de: &str) -> Option<&'static str> {
    match de {
        "KDE" | "Plasma" => Some("KWin"),
        "GNOME" => Some("Mutter"),
        "XFCE" => Some("xfwm4"),
        "Cinnamon" => Some("Muffin"),
        "MATE" => Some("Marco"),
        "LXQt" => Some("Openbox"),
        "Budgie" => Some("Mutter"),
        _ => None,
    }
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

/// GPU 粗版：PCI vendor 映射 + 内核驱动名（精确型号需要 pci.ids，不做）
fn gpu() -> Option<String> {
    let dir = std::fs::read_dir("/sys/class/drm").ok()?;
    let mut gpus: Vec<String> = Vec::new();
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // 只要 card0/card1…，排除 card0-DP-1 等连接器节点
        if !entry.path().join("device").exists()
            || !name
                .strip_prefix("card")
                .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let dev = entry.path().join("device");
        let vendor_raw = read(&format!("{}/vendor", dev.display()))?;
        let vendor = u16::from_str_radix(vendor_raw.trim_start_matches("0x"), 16).ok()?;
        let vendor_name = vendor_name(vendor).unwrap_or(vendor_raw.trim_start_matches("0x"));
        let driver = std::fs::read_link(format!("{}/driver", dev.display()))
            .ok()
            .map(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            });
        match driver {
            Some(d) => gpus.push(format!("{vendor_name} ({d})")),
            None => gpus.push(vendor_name.to_string()),
        }
    }
    (!gpus.is_empty()).then(|| gpus.join(", "))
}

/// PCI vendor id → 厂商名
fn vendor_name(id: u16) -> Option<&'static str> {
    match id {
        0x10de => Some("NVIDIA"),
        0x8086 => Some("Intel"),
        0x1002 | 0x1022 => Some("AMD"),
        0x15ad => Some("VMware"),
        0x1af4 => Some("virtio"),
        0x1234 => Some("Bochs"),
        _ => None,
    }
}

fn cpu() -> Option<String> {
    let info = read("/proc/cpuinfo")?;
    let model = info
        .lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())?;
    let cores = info.lines().filter(|l| l.starts_with("processor")).count();
    // P/E 核区分需要 sysfs core_type（本机内核未导出），先显示总核数
    let mut s = format!("{model} ({cores})");
    if let Some(khz) = max_cpu_freq() {
        s.push_str(&freq_str(khz));
    }
    Some(s)
}

/// 各 cpu 的 cpuinfo_max_freq（kHz）最大值
fn max_cpu_freq() -> Option<u64> {
    std::fs::read_dir("/sys/devices/system/cpu")
        .ok()?
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_prefix("cpu")
                .is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
        })
        .filter_map(|e| read(&format!("{}/cpufreq/cpuinfo_max_freq", e.path().display())))
        .filter_map(|v| v.parse().ok())
        .max()
}

fn freq_str(khz: u64) -> String {
    format!(" @ {:.2} GHz", khz as f64 / 1e6)
}

fn memory() -> Option<String> {
    let info = read("/proc/meminfo")?;
    let total = meminfo_kib(&info, "MemTotal")?;
    let avail = meminfo_kib(&info, "MemAvailable")?;
    Some(usage(total.saturating_sub(avail), total))
}

fn swap() -> Option<String> {
    let info = read("/proc/meminfo")?;
    let total = meminfo_kib(&info, "SwapTotal")?;
    if total == 0 {
        return None;
    }
    let free = meminfo_kib(&info, "SwapFree")?;
    Some(usage(total.saturating_sub(free), total))
}

/// Disk：手写 statvfs FFI（与 terminal_size 的 ioctl 同风格，零 crate）
fn disk() -> Option<String> {
    #[repr(C)]
    struct Statvfs {
        f_bsize: u64,
        f_frsize: u64,
        f_blocks: u64,
        f_bfree: u64,
        f_bavail: u64,
        f_files: u64,
        f_ffree: u64,
        f_favail: u64,
        f_fsid: u64,
        f_flag: u64,
        f_namemax: u64,
        __reserved: [u32; 3],
    }
    unsafe extern "C" {
        fn statvfs(path: *const std::os::raw::c_char, buf: *mut Statvfs) -> i32;
    }
    let mut buf: Statvfs = unsafe { std::mem::zeroed() };
    if unsafe { statvfs(c"/".as_ptr(), &mut buf) } != 0 {
        return None;
    }
    let unit = buf.f_frsize.max(buf.f_bsize);
    let total_kib = buf.f_blocks.saturating_mul(unit) / 1024;
    let avail_kib = buf.f_bavail.saturating_mul(unit) / 1024;
    let mut s = usage(total_kib.saturating_sub(avail_kib), total_kib);
    if let Some(t) = read("/proc/self/mountinfo").and_then(|m| fs_type_of(&m, "/")) {
        s.push_str(&format!(" - {t}"));
    }
    Some(s)
}

/// mountinfo 中指定挂载点的文件系统类型（"/" 无空格转义问题）
fn fs_type_of(mountinfo: &str, mount: &str) -> Option<String> {
    for line in mountinfo.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        if fields.get(4) != Some(&mount) {
            continue;
        }
        // 行尾 " - fstype source superoptions"；异常行跳过继续找
        let Some((_, post)) = line.split_once(" - ") else {
            continue;
        };
        return post.split(' ').next().map(str::to_string);
    }
    None
}

fn battery() -> Option<String> {
    let dir = std::fs::read_dir("/sys/class/power_supply").ok()?;
    let bat = dir.flatten().map(|e| e.path()).find(|p| {
        p.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("BAT"))
    })?;
    let cap = read(&format!("{}/capacity", bat.display()))?
        .parse::<u8>()
        .ok()?;
    let status = read(&format!("{}/status", bat.display())).unwrap_or_default();
    let ac = supply_online("AC")
        .or_else(|| supply_online("ADP"))
        .unwrap_or(false);
    let s = format!("{cap}% [{}]", battery_label(&status, ac));
    // 型号名（如 L21B4PC0）括注，对齐 fastfetch 的电池标识
    match read(&format!("{}/model_name", bat.display())).filter(|m| !m.is_empty()) {
        Some(model) => Some(format!("{s} ({model})")),
        None => Some(s),
    }
}

fn battery_label(status: &str, ac: bool) -> String {
    match status {
        "Charging" => "Charging".into(),
        "Discharging" => "Discharging".into(),
        "Full" => "Full".into(),
        _ if ac => "AC Connected".into(),
        _ => "Not charging".into(),
    }
}

/// 按名称前缀找电源供应节点（AC/ADP1 等）的 online 状态
fn supply_online(prefix: &str) -> Option<bool> {
    let dir = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for e in dir.flatten() {
        if e.file_name().to_string_lossy().starts_with(prefix) {
            return Some(read(&format!("{}/online", e.path().display()))? == "1");
        }
    }
    None
}

fn load() -> Option<String> {
    let t = read("/proc/loadavg")?;
    let parts: Vec<&str> = t.split_whitespace().collect();
    Some(format!(
        "{} {} {}",
        parts.first()?,
        parts.get(1)?,
        parts.get(2)?
    ))
}

fn locale() -> Option<String> {
    ["LC_ALL", "LANG"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
}

fn meminfo_kib(content: &str, field: &str) -> Option<u64> {
    content
        .lines()
        .find(|l| l.starts_with(field))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
}

// ---- 容量格式化（memory / swap / disk 共用） ----

/// KiB → 自适应单位：≥1 TiB 用 TiB，≥1 GiB 用 GiB，否则 MiB
fn human(kib: u64) -> String {
    let v = kib as f64;
    const GIB: f64 = (1u64 << 20) as f64;
    const TIB: f64 = (1u64 << 30) as f64;
    if v >= TIB {
        format!("{:.2} TiB", v / TIB)
    } else if v >= GIB {
        format!("{:.2} GiB", v / GIB)
    } else {
        format!("{:.2} MiB", v / (1u64 << 10) as f64)
    }
}

/// "已用 / 总量 (百分比)"
fn usage(used_kib: u64, total_kib: u64) -> String {
    let pct = if total_kib > 0 {
        used_kib as f64 / total_kib as f64 * 100.0
    } else {
        0.0
    };
    format!("{} / {} ({:.0}%)", human(used_kib), human(total_kib), pct)
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
    fn mountinfo_fs_type() {
        let sample = "36 35 0:35 / / rw,noatime shared:1 - btrfs /dev/nvme1n1p2 rw,ssd\n\
                      40 35 0:40 / /boot rw - ext4 /dev/nvme0n1p1 rw\n";
        assert_eq!(fs_type_of(sample, "/"), Some("btrfs".into()));
        assert_eq!(fs_type_of(sample, "/boot"), Some("ext4".into()));
        assert_eq!(fs_type_of(sample, "/nonexist"), None);
    }

    #[test]
    fn cpu_freq_format() {
        assert_eq!(freq_str(4500000), " @ 4.50 GHz");
        assert_eq!(freq_str(3200000), " @ 3.20 GHz");
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
    fn capacity_formatting() {
        // 16 GiB 总量、约 8.44 GiB 已用
        assert_eq!(usage(8845434, 16777216), "8.44 GiB / 16.00 GiB (53%)");
        assert_eq!(human(512 * 1024), "512.00 MiB");
        assert_eq!(human(3 * 512 * 1024), "1.50 GiB");
        assert_eq!(human(2 * (1 << 30)), "2.00 TiB");
    }

    #[test]
    fn de_wm_mapping() {
        assert_eq!(wm_name("KDE"), Some("KWin"));
        assert_eq!(wm_name("GNOME"), Some("Mutter"));
        assert_eq!(wm_name(" sway"), None);
    }

    #[test]
    fn gpu_vendor_mapping() {
        assert_eq!(vendor_name(0x10de), Some("NVIDIA"));
        assert_eq!(vendor_name(0x8086), Some("Intel"));
        assert_eq!(vendor_name(0x1002), Some("AMD"));
        assert_eq!(vendor_name(0xdead), None);
    }

    #[test]
    fn battery_labels() {
        assert_eq!(battery_label("Charging", false), "Charging");
        assert_eq!(battery_label("Not charging", true), "AC Connected");
        assert_eq!(battery_label("Not charging", false), "Not charging");
        assert_eq!(battery_label("Full", true), "Full");
    }

    #[test]
    fn resolve_selection() {
        // 默认集非空且全部合法
        assert!(resolve(None).is_ok_and(|names| !names.is_empty()));
        // 选择序无关、按注册表顺序展开、自动去重
        let sel = vec!["memory".to_string(), "os".to_string(), "memory".to_string()];
        assert_eq!(resolve(Some(&sel)).unwrap(), vec!["os", "memory"]);
        // 未知模块报错并附可用名单
        let err = resolve(Some(&["nope".to_string()])).unwrap_err();
        assert!(err.starts_with("未知模块: nope"));
    }
}
