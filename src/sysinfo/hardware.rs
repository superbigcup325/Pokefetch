// 硬件模块：GPU/CPU/内存/交换/Disk/电池，
// 数据来自 /sys（drm、cpufreq、power_supply）、/proc（cpuinfo、meminfo、
// mountinfo）、pci.ids（hwdata）与 statvfs FFI（声明见 crate::ffi）
use super::read;
use crate::ffi;

/// GPU：pci.ids 可查则显示营销型号，否则退回厂商+内核驱动名；
/// 多卡且存在 Intel 时对非 Intel 标 [Discrete]
pub(super) fn gpu() -> Option<String> {
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

pub(super) fn cpu() -> Option<String> {
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

pub(super) fn memory() -> Option<String> {
    let info = read("/proc/meminfo")?;
    let total = meminfo_kib(&info, "MemTotal")?;
    let avail = meminfo_kib(&info, "MemAvailable")?;
    Some(usage(total.saturating_sub(avail), total))
}

pub(super) fn swap() -> Option<String> {
    let info = read("/proc/meminfo")?;
    let total = meminfo_kib(&info, "SwapTotal")?;
    if total == 0 {
        return None;
    }
    let free = meminfo_kib(&info, "SwapFree")?;
    Some(usage(total.saturating_sub(free), total))
}

/// Disk：statvfs FFI（声明见 ffi.rs），报告根挂载点用量
pub(super) fn disk() -> Option<String> {
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

pub(super) fn battery() -> Option<String> {
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
    fn capacity_formatting() {
        // 16 GiB 总量、约 8.44 GiB 已用
        assert_eq!(usage(8845434, 16777216), "8.44 GiB / 16.00 GiB (53%)");
        assert_eq!(human(512 * 1024), "512.00 MiB");
        assert_eq!(human(3 * 512 * 1024), "1.50 GiB");
        assert_eq!(human(2 * (1 << 30)), "2.00 TiB");
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
}
