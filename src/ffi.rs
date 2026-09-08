// 手写 libc FFI 的唯一声明点：ioctl / socket / statvfs 等只在此处 extern 一次，
// 各功能（终端尺寸、Local IP、Disk）共用，避免同名符号多处声明后签名各自漂移
// （此前 main 与 sysinfo 各自声明 ioctl，签名不一致触发过 clashing_extern_declarations）。
// 结构体布局按 x86_64 Linux 手抄，只依赖 std。

use std::ffi::{c_char, c_void};

/// struct ifreq：name[16] + ifru union[24]（x86_64 上 sockaddr/ifmap 均装得下）
#[repr(C)]
pub struct IfReq {
    pub name: [u8; 16],
    pub data: [u8; 24],
}

/// struct ifconf：SIOCGIFCONF 的缓冲描述
#[repr(C)]
pub struct IfConf {
    pub len: i32,
    pub ptr: *mut IfReq,
}

/// struct statvfs（x86_64 Linux）
#[repr(C)]
pub struct Statvfs {
    pub f_bsize: u64,
    pub f_frsize: u64,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_favail: u64,
    pub f_fsid: u64,
    pub f_flag: u64,
    pub f_namemax: u64,
    __reserved: [u32; 3],
}

unsafe extern "C" {
    pub fn open(path: *const c_char, flags: i32) -> i32;
    pub fn close(fd: i32) -> i32;
    pub fn ioctl(fd: i32, request: u64, arg: *mut c_void) -> i32;
    pub fn socket(domain: i32, ty: i32, proto: i32) -> i32;
    pub fn statvfs(path: *const c_char, buf: *mut Statvfs) -> i32;
}

pub const TIOCGWINSZ: u64 = 0x5413;
pub const SIOCGIFCONF: u64 = 0x8912;
pub const SIOCGIFNETMASK: u64 = 0x891b;

/// 终端尺寸（行, 列）。优先查控制终端 /dev/tty——stdout 被管道接管
/// （fastfetch 注入场景）时它才是最终显示窗口；再退标准流；都不是 tty 返回 None
pub fn terminal_size() -> Option<(usize, usize)> {
    #[repr(C)]
    struct Winsize {
        rows: u16,
        cols: u16,
        xpix: u16,
        ypix: u16,
    }
    const O_RDWR: i32 = 2;
    let query = |fd: i32| unsafe {
        let mut ws = Winsize {
            rows: 0,
            cols: 0,
            xpix: 0,
            ypix: 0,
        };
        if ioctl(
            fd,
            TIOCGWINSZ,
            &mut ws as *mut Winsize as *mut std::ffi::c_void,
        ) == 0
            && ws.rows > 0
            && ws.cols > 0
        {
            Some((usize::from(ws.rows), usize::from(ws.cols)))
        } else {
            None
        }
    };
    unsafe {
        let fd = open(c"/dev/tty".as_ptr(), O_RDWR);
        if fd >= 0 {
            let size = query(fd);
            close(fd);
            if size.is_some() {
                return size;
            }
        }
    }
    [1, 0, 2].into_iter().find_map(query)
}
