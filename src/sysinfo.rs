// 系统信息数据层：读 /proc、/sys 与环境变量，纯 std 零依赖。
// 信息缺失一律返回 None（对应行被跳过），不引入 panic 路径。
//
// 结构：MODULES 注册表是单一事实源——模块名、取值函数、展开顺序、
// 默认集与 --modules 校验全部由它派生；collect 只按解析出的名单展开。
// 手写 libc FFI（Local IP / Disk 用）统一声明在 crate::ffi。

use crate::ffi::{self, IfConf, IfReq, SIOCGIFCONF, SIOCGIFNETMASK};

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
    ("wmtheme", "WM Theme", wm_theme),
    ("theme", "Theme", theme),
    ("icons", "Icons", icons),
    ("font", "Font", font),
    ("cursor", "Cursor", cursor),
    ("terminal", "Terminal", terminal),
    ("gpu", "GPU", gpu),
    ("cpu", "CPU", cpu),
    ("memory", "Memory", memory),
    ("swap", "Swap", swap),
    ("disk", "Disk", disk),
    ("localip", "Local IP", local_ip),
    ("localip-all", "Local IP (all)", local_ip_all),
    ("battery", "Battery", battery),
    ("load", "Load", load),
    ("locale", "Locale", locale),
];

/// 默认集（面板精选；board/bios 与 localip-all、KDE 主题五件套经 --modules 点名启用）
const DEFAULT_SELECTED: &[&str] = &[
    "os", "host", "kernel", "uptime", "packages", "shell", "de", "wm", "terminal", "gpu", "cpu",
    "memory", "swap", "disk", "localip", "battery", "load", "locale",
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
    if base.is_empty() {
        return None;
    }
    // 版本走 pacman 包库（可执行名≈包名），非 Arch 或查不到时只显示名字
    match pacman_version(base) {
        Some(v) => Some(format!("{base} {v}")),
        None => Some(base.to_string()),
    }
}

fn de() -> Option<String> {
    let d = std::env::var("XDG_CURRENT_DESKTOP").ok()?;
    let name = d.split(':').find(|s| !s.is_empty())?;
    let mut s = name.to_string();
    match name {
        // 版本号零依赖来源：KDE 走 pacman 包库，GNOME 走版本 xml
        "KDE" => {
            if let Some(v) = pacman_version("plasma-workspace") {
                s = format!("{name} Plasma {v}");
            }
        }
        "GNOME" => {
            if let Some(v) = gnome_version() {
                s = format!("{name} {v}");
            }
        }
        _ => {}
    }
    Some(s)
}

/// pacman 本地包库 %VERSION%（剥掉 pkgrel）
fn pacman_version(pkg: &str) -> Option<String> {
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

/// /usr/share/gnome/gnome-version.xml → "47.2"
fn gnome_version() -> Option<String> {
    let xml = read("/usr/share/gnome/gnome-version.xml")?;
    gnome_version_from(&xml)
}

fn gnome_version_from(xml: &str) -> Option<String> {
    let tag = |t: &str| -> Option<String> {
        let (open, close) = (format!("<{t}>"), format!("</{t}>"));
        let start = xml.find(&open)? + open.len();
        let end = xml[start..].find(&close)? + start;
        Some(xml[start..end].to_string())
    };
    let mut v = tag("platform")?;
    for part in ["minor", "micro"] {
        if let Some(x) = tag(part) {
            v.push('.');
            v.push_str(&x);
        }
    }
    Some(v)
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

// ---- KDE 主题五件套：kdeglobals / kwinrc ini 解析（非 KDE 环境文件缺失自动跳行） ----

/// ini 文本中 [section] 段的 key 值（KDE 配置即此形式，不做转义处理）
fn ini_get<'a>(content: &'a str, section: &str, key: &str) -> Option<&'a str> {
    let mut in_section = false;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == format!("[{section}]");
        } else if in_section
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            return Some(v.trim());
        }
    }
    None
}

/// XDG 配置目录下的文件路径
fn config_path(file: &str) -> Option<String> {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| std::path::PathBuf::from(h).join(".config"))
        })?;
    Some(base.join(file).display().to_string())
}

/// KDE 配置文件某段的某个键（空值视同缺失）
fn kde_ini(file: &str, section: &str, key: &str) -> Option<String> {
    let content = read(&config_path(file)?)?;
    ini_get(&content, section, key)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn wm_theme() -> Option<String> {
    let t = kde_ini("kwinrc", "org.kde.kdecoration2", "theme")?;
    // aurorae 主题存的是 "__aurorae__svg__WhiteSur-dark"，剥掉内部前缀
    Some(t.strip_prefix("__aurorae__svg__").unwrap_or(&t).to_string())
}

fn theme() -> Option<String> {
    kde_ini("kdeglobals", "General", "ColorScheme")
}

fn icons() -> Option<String> {
    kde_ini("kdeglobals", "Icons", "Theme")
}

fn cursor() -> Option<String> {
    // KDE 链：用户 kcminputrc → 发行版默认 kdedefaults/kcminputrc
    let theme = kde_ini("kcminputrc", "Mouse", "cursorTheme")
        .or_else(|| kde_ini("kdedefaults/kcminputrc", "Mouse", "cursorTheme"))?;
    // "WhiteSur-cursors" 这类包名习惯剥掉 -cursors 后缀
    let theme = theme.strip_suffix("-cursors").unwrap_or(&theme).to_string();
    let size = kde_ini("kcminputrc", "Mouse", "cursorSize")
        .or_else(|| kde_ini("kdedefaults/kcminputrc", "Mouse", "cursorSize"));
    Some(match size {
        Some(s) => format!("{theme} ({s}px)"),
        None => theme,
    })
}

fn font() -> Option<String> {
    kde_ini("kdeglobals", "General", "font").and_then(|f| qt_font_str(&f))
}

/// Qt 字体串 "Inter,11,-1,5,..." → "Inter (11pt)"
fn qt_font_str(s: &str) -> Option<String> {
    let mut it = s.split(',');
    let family = it.next()?.trim().to_string();
    let pt = it.next()?.trim().parse::<u8>().ok()?;
    (!family.is_empty()).then(|| format!("{family} ({pt}pt)"))
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

/// GPU：pci.ids 可查则显示营销型号，否则退回厂商+内核驱动名；
/// 多卡且存在 Intel 时对非 Intel 标 [Discrete]
fn gpu() -> Option<String> {
    let dir = std::fs::read_dir("/sys/class/drm").ok()?;
    let ids = pci_ids();
    let mut gpus: Vec<(String, u16)> = Vec::new();
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
        let (Some(vendor_raw), Some(device_raw)) = (
            read(&format!("{}/vendor", dev.display())),
            read(&format!("{}/device", dev.display())),
        ) else {
            continue;
        };
        let Ok(vendor) = u16::from_str_radix(vendor_raw.trim_start_matches("0x"), 16) else {
            continue;
        };
        let device = u16::from_str_radix(device_raw.trim_start_matches("0x"), 16).ok();
        let vname = vendor_name(vendor).unwrap_or(vendor_raw.trim_start_matches("0x"));
        // pci.ids 可查则显示营销型号，否则退回内核驱动名
        let label = match device.and_then(|d| {
            ids.as_deref()
                .and_then(|ids| pci_device_name(ids, vendor, d))
        }) {
            Some(model) => format!("{vname} {model}"),
            None => {
                let driver = std::fs::read_link(format!("{}/driver", dev.display()))
                    .ok()
                    .map(|p| {
                        p.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string()
                    });
                match driver {
                    Some(d) => format!("{vname} ({d})"),
                    None => vname.to_string(),
                }
            }
        };
        gpus.push((label, vendor));
    }
    if gpus.len() > 1 && gpus.iter().any(|(_, v)| *v == 0x8086) {
        for (label, v) in &mut gpus {
            if *v != 0x8086 {
                label.push_str(" [Discrete]");
            }
        }
    }
    (!gpus.is_empty()).then(|| {
        gpus.into_iter()
            .map(|(l, _)| l)
            .collect::<Vec<_>>()
            .join(", ")
    })
}

/// 系统 pci.ids 数据库（hwdata），缺失则 GPU 降级粗版
fn pci_ids() -> Option<String> {
    ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
}

/// pci.ids 文本中查 vendor:device 的设备名；"GA107M [GeForce RTX 3050 Mobile]"
/// 这类带方括号的名字取括号内营销名
fn pci_device_name(content: &str, vendor: u16, device: u16) -> Option<String> {
    let vs = format!("{vendor:04x}");
    let ds = format!("{device:04x}");
    let mut in_vendor = false;
    for line in content.lines() {
        // 空行与注释行（vendor 段内部也有）不改变段落状态
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !line.starts_with('\t') {
            in_vendor = line
                .split_whitespace()
                .next()
                .is_some_and(|id| id.eq_ignore_ascii_case(&vs));
            continue;
        }
        if !in_vendor || line.starts_with("\t\t") {
            continue; // 子系统等二级条目不查
        }
        let body = line.trim_start();
        if body.split_whitespace().next().is_some_and(|id| id == ds) {
            let name = body[ds.len()..].trim();
            return Some(match (name.find('['), name.ends_with(']')) {
                (Some(i), true) => name[i + 1..name.len() - 1].to_string(),
                _ => name.to_string(),
            });
        }
    }
    None
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

/// Disk：statvfs FFI（声明见 ffi.rs），报告根挂载点用量
fn disk() -> Option<String> {
    let mut buf: ffi::Statvfs = unsafe { std::mem::zeroed() };
    if unsafe { ffi::statvfs(c"/".as_ptr(), &mut buf) } != 0 {
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

/// Local IP 默认只列物理网卡：隧道/容器等虚拟接口按名过滤，行宽不随 VPN 失控；
/// 过滤后为空（纯隧道环境）回退全量，超过预算项数以 …+k 截尾
const LOCALIP_BUDGET: usize = 3;

/// 隧道/虚拟网络接口名前缀（VPN、容器网桥、veth 对端、虚拟机宿主桥等）；
/// 保守清单，遇到新的 VPN 工具可再扩
const VIRTUAL_IF_PREFIXES: [&str; 20] = [
    "tun",
    "tap",
    "wg",
    "zt",
    "tailscale",
    "docker",
    "virbr",
    "veth",
    "br-",
    "vmnet",
    "vboxnet",
    "ppp",
    "ipsec",
    "nordlynx",
    "proton",
    "mullvad",
    "podman",
    "cni",
    "flannel",
    "cali",
];

fn is_virtual_if(name: &str) -> bool {
    VIRTUAL_IF_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// 超预算截尾：前 limit 项 + "…+k"
fn join_with_budget(list: &[String], limit: usize) -> String {
    if list.len() <= limit {
        list.join(", ")
    } else {
        format!("{}, …+{}", list[..limit].join(", "), list.len() - limit)
    }
}

/// Local IP：SIOCGIFCONF/SIOCGIFNETMASK（FFI 声明见 ffi.rs）；
/// localip = 仅物理网卡（含预算截尾），localip-all = 全量
fn local_ip() -> Option<String> {
    local_ip_impl(true)
}

fn local_ip_all() -> Option<String> {
    local_ip_impl(false)
}

fn local_ip_impl(only_physical: bool) -> Option<String> {
    let fd = unsafe {
        ffi::socket(2 /* AF_INET */, 2 /* SOCK_DGRAM */, 0)
    };
    if fd < 0 {
        return None;
    }
    let work = (|| {
        // SIOCGIFCONF：缓冲不够时内核截断不报错，装满就翻倍重试
        let mut cap: usize = 4096;
        let entries = loop {
            let mut buf = vec![0u8; cap];
            let mut ifc = IfConf {
                len: cap as i32,
                ptr: buf.as_mut_ptr() as *mut IfReq,
            };
            if unsafe {
                ffi::ioctl(
                    fd,
                    SIOCGIFCONF,
                    &mut ifc as *mut IfConf as *mut std::ffi::c_void,
                )
            } != 0
            {
                return None;
            }
            let used = ifc.len as usize;
            if used + std::mem::size_of::<IfReq>() <= cap {
                break ifreq_entries(&buf[..used]);
            }
            cap *= 2;
            if cap > (1 << 20) {
                return None;
            }
        };
        let mut items: Vec<(String, String)> = Vec::new();
        for (name, ip) in entries {
            if name == "lo" {
                continue;
            }
            let mut req = IfReq {
                name: [0; 16],
                data: [0; 24],
            };
            req.name[..name.len()].copy_from_slice(name.as_bytes());
            if unsafe {
                ffi::ioctl(
                    fd,
                    SIOCGIFNETMASK,
                    &mut req as *mut IfReq as *mut std::ffi::c_void,
                )
            } != 0
            {
                continue;
            }
            let mask = [req.data[4], req.data[5], req.data[6], req.data[7]];
            let entry = format!("{name}: {}/{}", ipv4_str(ip), prefix_of(mask));
            items.push((name, entry));
        }
        let list: Vec<String> = if only_physical {
            let physical: Vec<String> = items
                .iter()
                .filter(|(n, _)| !is_virtual_if(n))
                .map(|(_, s)| s.clone())
                .collect();
            if physical.is_empty() {
                items.into_iter().map(|(_, s)| s).collect()
            } else {
                physical
            }
        } else {
            items.into_iter().map(|(_, s)| s).collect()
        };
        (!list.is_empty()).then(|| {
            if only_physical {
                join_with_budget(&list, LOCALIP_BUDGET)
            } else {
                list.join(", ")
            }
        })
    })();
    unsafe { ffi::close(fd) };
    work
}

/// 解析 SIOCGIFCONF 缓冲：40B 定长 ifreq，name[16] + sockaddr union[24]，
/// 仅保留 AF_INET 条目
fn ifreq_entries(buf: &[u8]) -> Vec<(String, [u8; 4])> {
    let stride = std::mem::size_of::<IfReq>();
    buf.chunks_exact(stride)
        .filter_map(|r| {
            let family = u16::from_le_bytes([r[16], r[17]]);
            if family != 2 {
                return None;
            }
            let name_end = r[..16].iter().position(|&b| b == 0).unwrap_or(16);
            let name = String::from_utf8_lossy(&r[..name_end]).to_string();
            Some((name, [r[20], r[21], r[22], r[23]]))
        })
        .collect()
}

fn ipv4_str(b: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

fn prefix_of(netmask: [u8; 4]) -> u32 {
    u32::from_be_bytes(netmask).count_ones()
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
    fn ifconf_buffer_parsing() {
        // 两条 40B ifreq：eth0=AF_INET 192.168.1.8，lo=AF_INET 127.0.0.1
        let mut buf = vec![0u8; 80];
        buf[..4].copy_from_slice(b"eth0");
        buf[16..18].copy_from_slice(&2u16.to_le_bytes());
        buf[20..24].copy_from_slice(&[192, 168, 1, 8]);
        buf[40..42].copy_from_slice(b"lo");
        buf[56..58].copy_from_slice(&2u16.to_le_bytes());
        buf[60..64].copy_from_slice(&[127, 0, 0, 1]);
        let entries = ifreq_entries(&buf);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], ("eth0".to_string(), [192, 168, 1, 8]));
        // 非 AF_INET 条目被过滤
        buf[16..18].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(ifreq_entries(&buf).len(), 1);
    }

    #[test]
    fn netmask_prefix() {
        assert_eq!(prefix_of([255, 255, 255, 0]), 24);
        assert_eq!(prefix_of([255, 255, 252, 0]), 22);
        assert_eq!(prefix_of([0, 0, 0, 0]), 0);
        assert_eq!(ipv4_str([172, 29, 31, 103]), "172.29.31.103");
    }

    #[test]
    fn pci_ids_lookup() {
        let ids = "10de  NVIDIA Corporation\n\
                   \t25a2  GA107M [GeForce RTX 3050 Mobile]\n\
                   \t\tff00  device\n\
                   8086  Intel Corporation\n\
                   \t46a6  Alder Lake-UP3 GT2 [Iris Xe]\n";
        assert_eq!(
            pci_device_name(ids, 0x10de, 0x25a2),
            Some("GeForce RTX 3050 Mobile".into())
        );
        assert_eq!(pci_device_name(ids, 0x8086, 0x46a6), Some("Iris Xe".into()));
        assert_eq!(pci_device_name(ids, 0x10de, 0xffff), None);
    }

    #[test]
    fn ini_lookup() {
        let cfg = "[General]\nColorScheme=NimbusRefinedDark\nfont=Inter,11,-1,5\n\n[Icons]\nTheme=WhiteSur\n";
        assert_eq!(
            ini_get(cfg, "General", "ColorScheme"),
            Some("NimbusRefinedDark")
        );
        assert_eq!(ini_get(cfg, "Icons", "Theme"), Some("WhiteSur"));
        assert_eq!(ini_get(cfg, "General", "missing"), None);
        assert_eq!(ini_get(cfg, "NoSection", "ColorScheme"), None);
    }

    #[test]
    fn qt_font_parsing() {
        assert_eq!(
            qt_font_str("Inter,11,-1,5,50,0"),
            Some("Inter (11pt)".into())
        );
        assert_eq!(qt_font_str(",10,x"), None);
    }

    #[test]
    fn pacman_version_parsing() {
        let desc = "%NAME%\nplasma-workspace\n\n%VERSION%\n6.7.4-1\n\n%BASE%\nx\n";
        assert_eq!(pacman_version_inner(desc), Some("6.7.4".into()));
    }

    #[test]
    fn gnome_version_parsing() {
        let xml = "<gnome-version><platform>47</platform><minor>2</minor><micro>1</micro></gnome-version>";
        assert_eq!(gnome_version_from(xml), Some("47.2.1".into()));
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

    #[test]
    fn localip_virtual_if_names() {
        assert!(!is_virtual_if("enp49s0"));
        assert!(!is_virtual_if("wlan0"));
        assert!(!is_virtual_if("eth0"));
        assert!(is_virtual_if("tun0"));
        assert!(is_virtual_if("wg0"));
        assert!(is_virtual_if("ztfcazsbm3"));
        assert!(is_virtual_if("tailscale0"));
        assert!(is_virtual_if("docker0"));
        assert!(is_virtual_if("veth8a2c1b@if5"));
        assert!(is_virtual_if("br-1a2b3c4d"));
    }

    #[test]
    fn localip_budget_truncation() {
        let two: Vec<String> = vec!["a".into(), "b".into()];
        assert_eq!(join_with_budget(&two, 3), "a, b");
        let five: Vec<String> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(join_with_budget(&five, 3), "a, b, c, …+2");
    }

    #[test]
    fn localip_all_module_registered() {
        // 注册表里 localip 与 localip-all 并存，后者不在默认集
        let names: Vec<&str> = module_names().collect();
        assert!(names.contains(&"localip") && names.contains(&"localip-all"));
        assert!(DEFAULT_SELECTED.contains(&"localip"));
        assert!(!DEFAULT_SELECTED.contains(&"localip-all"));
        assert_eq!(
            resolve(Some(&["localip-all".to_string()])).unwrap(),
            vec!["localip-all"]
        );
    }
}
