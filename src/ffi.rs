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

/// struct sigaction（glibc/x86_64 布局：handler + mask + flags + restorer，
/// 注意与内核 rt_sigaction 的字段顺序不同）。restorer 由 glibc 包装函数自动补
#[repr(C)]
pub struct SigAction {
    pub handler: usize,
    pub mask: [u64; 16],
    pub flags: i32,
    pub restorer: usize,
}

unsafe extern "C" {
    pub fn open(path: *const c_char, flags: i32) -> i32;
    pub fn close(fd: i32) -> i32;
    pub fn read(fd: i32, buf: *mut c_void, count: usize) -> isize;
    pub fn ioctl(fd: i32, request: u64, arg: *mut c_void) -> i32;
    pub fn socket(domain: i32, ty: i32, proto: i32) -> i32;
    pub fn statvfs(path: *const c_char, buf: *mut Statvfs) -> i32;
    pub fn isatty(fd: i32) -> i32;
    pub fn tcgetattr(fd: i32, termios: *mut Termios) -> i32;
    pub fn tcsetattr(fd: i32, actions: i32, termios: *const Termios) -> i32;
    pub fn sigaction(signum: i32, act: *const SigAction, old: *mut SigAction) -> i32;
    pub fn signal(signum: i32, handler: usize) -> usize;
}

/// SIGPIPE 的默认处置（SIG_DFL）：进程收到信号即终止
const SIG_DFL: usize = 0;

/// SIGPIPE（x86_64 Linux）
pub const SIGPIPE: i32 = 13;

/// 恢复 SIGPIPE 默认处置。Rust 运行时启动时忽略 SIGPIPE，下游提前关闭的
/// 管道（pokefetch … | head）会让 write 返回 EPIPE、println! 直接 panic 刷
/// 回溯；恢复后进程按 Unix 惯例被 SIGPIPE 终止（shell 报 141）。须在
/// main 最开头调用
pub fn restore_sigpipe_default() {
    unsafe {
        signal(SIGPIPE, SIG_DFL);
    }
}

/// struct termios（x86_64 Linux）
#[repr(C)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line: u8,
    pub c_cc: [u8; 32],
    pub c_ispeed: u32,
    pub c_ospeed: u32,
}

pub const TCSANOW: i32 = 0;
pub const ICANON: u32 = 0x2;
pub const ECHO: u32 = 0x8;
pub const ISIG: u32 = 0x1;
pub const VMIN: usize = 6;
pub const VTIME: usize = 5;

pub fn stdout_is_tty() -> bool {
    unsafe { isatty(1) == 1 }
}

pub fn stdin_is_tty() -> bool {
    unsafe { isatty(0) == 1 }
}

/// stdin 原始模式守卫：关掉规范模式与回显（c_lflag），Drop 时恢复原 termios。
/// 两种读法：enable() 非阻塞轮询（VMIN=0，动画播放）；enable_blocking()
/// 阻塞读（VMIN=1，--watch 专用，0 返回即真 EOF、信号打断返回 EINTR）。
/// Drop 顺序契约见 play.rs / watch.rs 模块头
pub struct RawMode {
    saved: Termios,
}

impl RawMode {
    pub fn enable() -> Option<RawMode> {
        Self::setup(0)
    }

    pub fn enable_blocking() -> Option<RawMode> {
        Self::setup(1)
    }

    fn setup(vmin: u8) -> Option<RawMode> {
        unsafe {
            let mut t = std::mem::zeroed::<Termios>();
            if tcgetattr(0, &mut t) != 0 {
                return None;
            }
            let saved = Termios {
                c_iflag: t.c_iflag,
                c_oflag: t.c_oflag,
                c_cflag: t.c_cflag,
                c_lflag: t.c_lflag,
                c_line: t.c_line,
                c_cc: t.c_cc,
                c_ispeed: t.c_ispeed,
                c_ospeed: t.c_ospeed,
            };
            // ISIG 一并关掉：Ctrl-C 变成可读的 0x03 字节走优雅退出路径，
            // 否则 SIGINT 直接杀进程，termios 无法恢复，终端残留 raw 模式
            t.c_lflag &= !(ICANON | ECHO | ISIG);
            t.c_cc[VMIN] = vmin;
            t.c_cc[VTIME] = 0;
            if tcsetattr(0, TCSANOW, &t) != 0 {
                return None;
            }
            Some(RawMode { saved })
        }
    }

    /// 非阻塞读一个键（配合 enable()）；无输入返回 None
    pub fn read_key(&self) -> Option<u8> {
        let mut b = 0u8;
        let n = unsafe { read(0, &mut b as *mut u8 as *mut c_void, 1) };
        if n == 1 { Some(b) } else { None }
    }

    /// 阻塞读一字节（配合 enable_blocking()）：Some(Some(b))=按键；
    /// Some(None)=EOF（stdin 对端关闭）；None=被信号打断（EINTR），
    /// 调用方应先查 WINCH 标志再重读。VMIN=1 下 0 返回不可能是"无数据"，
    /// 只能是对端关闭，与非阻塞轮询的语义严格区分
    pub fn read_blocking(&self) -> Option<Option<u8>> {
        let mut b = 0u8;
        let n = unsafe { read(0, &mut b as *mut u8 as *mut c_void, 1) };
        match n {
            1 => Some(Some(b)),
            0 => Some(None),
            _ => None,
        }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        unsafe { tcsetattr(0, TCSANOW, &self.saved) };
    }
}

pub const TIOCGWINSZ: u64 = 0x5413;
pub const SIOCGIFCONF: u64 = 0x8912;
pub const SIOCGIFNETMASK: u64 = 0x891b;

/// 终端尺寸变化信号（x86_64 Linux）
pub const SIGWINCH: i32 = 28;

/// 注册 SIGWINCH 处理器。必须走 sigaction 而非 signal()：后者固定带
/// SA_RESTART，阻塞读不会被信号打断；这里 flags=0，resize 时阻塞中的
/// read 以 EINTR 醒来，调用方查标志重绘。处理器内只允许原子置位
/// （async-signal-safe），重绘动作由主循环执行
pub fn set_winch_handler(handler: extern "C" fn(i32)) {
    let act = SigAction {
        handler: handler as usize,
        mask: [0; 16],
        flags: 0,
        restorer: 0,
    };
    unsafe {
        sigaction(SIGWINCH, &act, std::ptr::null_mut());
    }
}

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
