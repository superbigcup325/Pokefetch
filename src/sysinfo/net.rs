// 网络模块：Local IP 经 SIOCGIFCONF/SIOCGIFNETMASK 枚举 IPv4 接口，
// localip 默认只列物理网卡（行宽不随 VPN 失控），localip-all 全量
use crate::ffi::{self, IfConf, IfReq, SIOCGIFCONF, SIOCGIFNETMASK};

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
pub(super) fn local_ip() -> Option<String> {
    local_ip_impl(true)
}

pub(super) fn local_ip_all() -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
