// 桌面环境模块：DE/WM 版本与 KDE 主题五件套，
// 数据来自环境变量、pacman 包库、gnome-version.xml 与 KDE ini 配置
use super::misc::pacman_version;
use super::read;

pub(super) fn de() -> Option<String> {
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

pub(super) fn wm() -> Option<String> {
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

pub(super) fn wm_theme() -> Option<String> {
    let t = kde_ini("kwinrc", "org.kde.kdecoration2", "theme")?;
    // aurorae 主题存的是 "__aurorae__svg__WhiteSur-dark"，剥掉内部前缀
    Some(t.strip_prefix("__aurorae__svg__").unwrap_or(&t).to_string())
}

pub(super) fn theme() -> Option<String> {
    kde_ini("kdeglobals", "General", "ColorScheme")
}

pub(super) fn icons() -> Option<String> {
    kde_ini("kdeglobals", "Icons", "Theme")
}

pub(super) fn cursor() -> Option<String> {
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

pub(super) fn font() -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn gnome_version_parsing() {
        let xml = "<gnome-version><platform>47</platform><minor>2</minor><micro>1</micro></gnome-version>";
        assert_eq!(gnome_version_from(xml), Some("47.2.1".into()));
    }

    #[test]
    fn de_wm_mapping() {
        assert_eq!(wm_name("KDE"), Some("KWin"));
        assert_eq!(wm_name("GNOME"), Some("Mutter"));
        assert_eq!(wm_name(" sway"), None);
    }
}
