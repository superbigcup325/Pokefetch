// 系统信息数据层：读 /proc、/sys 与环境变量，纯 std 零依赖。
// 信息缺失一律返回 None（对应行被跳过），不引入 panic 路径。
//
// 结构：MODULES 注册表是单一事实源——模块名、取值函数、展开顺序、
// 默认集与 --modules 校验全部由它派生；collect 只按解析出的名单展开。
// 取值函数按数据域分文件：misc 系统杂项 / desktop 桌面环境 / hardware 硬件
// / net 网络；共享小工具（read 等）留在本模块，子模块经 super:: 引用。
// 手写 libc FFI（Local IP / Disk 用）统一声明在 crate::ffi。

mod desktop;
mod hardware;
mod misc;
mod net;

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
    ("os", "OS", misc::os),
    ("host", "Host", misc::host),
    ("board", "Board", misc::board),
    ("bios", "BIOS", misc::bios),
    ("kernel", "Kernel", misc::kernel),
    ("uptime", "Uptime", misc::uptime),
    ("packages", "Packages", misc::packages),
    ("shell", "Shell", misc::shell),
    ("de", "DE", desktop::de),
    ("wm", "WM", desktop::wm),
    ("wmtheme", "WM Theme", desktop::wm_theme),
    ("theme", "Theme", desktop::theme),
    ("icons", "Icons", desktop::icons),
    ("font", "Font", desktop::font),
    ("cursor", "Cursor", desktop::cursor),
    ("terminal", "Terminal", misc::terminal),
    ("gpu", "GPU", hardware::gpu),
    ("cpu", "CPU", hardware::cpu),
    ("memory", "Memory", hardware::memory),
    ("swap", "Swap", hardware::swap),
    ("disk", "Disk", hardware::disk),
    ("localip", "Local IP", net::local_ip),
    ("localip-all", "Local IP (all)", net::local_ip_all),
    ("battery", "Battery", hardware::battery),
    ("load", "Load", misc::load),
    ("locale", "Locale", misc::locale),
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

#[cfg(test)]
mod tests {
    use super::*;

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
