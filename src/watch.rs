// --watch 常驻重绘：SIGWINCH 热加载。
//
// 进程常驻备用屏缓冲（ESC[?1049h），终端每次 resize（含 niri 等合成器里
// 全屏/平铺切换引起的 pty 尺寸变化）内核都会向前台进程组发 SIGWINCH：
// 处理器只置原子标志，阻塞中的 read 以 EINTR 醒来（sigaction 无
// SA_RESTART），主循环随即按新 ioctl 尺寸整帧重排——画布、面板预算、
// 精灵池过滤口径与一次性输出完全一致。
//
// RawMode 守卫生命周期契约与 play.rs 相同：run() 内创建、随作用域 Drop
// （先 tcflush 清未读输入再还原 termios），正常返回 / break / panic unwind
// 全覆盖；q/Q/Esc/Ctrl-C 退出、r/R 重 roll、stdin EOF（终端关闭）自动收场，
// 退出前还原主屏不污染 scrollback。
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::canvas::{compose, max_visible_width, pad_canvas, resolve_canvas};
use crate::ffi;
use crate::panel;
use crate::sysinfo;

static WINCH: AtomicBool = AtomicBool::new(false);

extern "C" fn on_winch(_: i32) {
    WINCH.store(true, Ordering::Relaxed);
}

/// 一次输出所需的最小数据：名字/shiny 供标题与 Dex 行，ansi 为精灵本体
pub(crate) struct Frame {
    pub(crate) name: String,
    pub(crate) shiny: bool,
    pub(crate) ansi: String,
}

/// 重绘上下文：与一次性输出共享的布局决策输入
pub(crate) struct Ctx {
    pub(crate) big: bool,
    pub(crate) canvas: Option<usize>,
    pub(crate) center: bool,
    pub(crate) no_panel: bool,
    pub(crate) show_title: bool,
    pub(crate) budget_enabled: bool,
    pub(crate) module_names: Vec<&'static str>,
}

/// 按当前终端尺寸渲染一整帧（标题行 + 精灵 + 面板），口径与 main 一次性输出一致
fn render(ctx: &Ctx, term: Option<(usize, usize)>, frame: &Frame) -> String {
    let cols = term.map(|(_, c)| c);
    let raw_w = max_visible_width(&frame.ansi);
    let canvas_w = resolve_canvas(ctx.canvas, ctx.big, cols);
    let padded = if canvas_w > 0 {
        pad_canvas(&frame.ansi, canvas_w, ctx.center, raw_w)
    } else {
        frame.ansi.clone()
    };
    let sprite_w = raw_w.max(canvas_w);
    let mut out = String::new();
    if ctx.show_title {
        out.push_str(&frame.name);
        if frame.shiny {
            out.push_str(" (shiny)");
        }
        out.push('\n');
    }
    // 面板让位规则与一次性输出一致：no_panel 或预算 0（终端只够精灵区）都只出精灵
    let budget = if ctx.no_panel {
        None
    } else {
        panel::row_budget(ctx.budget_enabled, cols, sprite_w)
    };
    if ctx.no_panel || matches!(budget, Some(0)) {
        out.push_str(&padded);
        return out;
    }
    let info = sysinfo::collect(&ctx.module_names);
    let dex = crate::dex_number(&frame.name).map(|num| {
        format!(
            "\x1b[1;34mDex:\x1b[0m #{num:03}{}",
            if frame.shiny { " ✨" } else { "" }
        )
    });
    let rows = panel::panel_rows(&info, budget, dex);
    out.push_str(&compose(&padded, rows, sprite_w, 3));
    out
}

/// 清屏并重画一帧；stdout 写失败（终端关闭/EPIPE）返回 false
fn draw(out: &mut impl Write, ctx: &Ctx, term: Option<(usize, usize)>, frame: &Frame) -> bool {
    let body = render(ctx, term, frame);
    out.write_all(b"\x1b[H\x1b[2J").is_ok()
        && out.write_all(body.as_bytes()).is_ok()
        && out.flush().is_ok()
}

/// 主循环：初始绘制一帧后阻塞在按键读取上；SIGWINCH（EINTR 唤醒）重排、
/// q/Esc/Ctrl-C 退出、r 重 roll、stdin EOF 自动收场。
/// 只在 r 重 roll 与首次绘制时调用 next（重掷精灵）
pub(crate) fn run(ctx: &Ctx, next: &mut dyn FnMut(Option<usize>) -> Frame) {
    let mut out = std::io::stdout().lock();
    // 备用屏缓冲：进入即清屏；若 stdout 已不可写则直接放弃
    if out
        .write_all(b"\x1b[?1049h\x1b[H\x1b[2J")
        .and_then(|_| out.flush())
        .is_err()
    {
        return;
    }
    let raw = ffi::RawMode::enable_blocking();
    ffi::set_winch_handler(on_winch);
    let term = ffi::terminal_size();
    let mut frame = next(term.map(|(_, cols)| cols));
    let mut alive = draw(&mut out, ctx, term, &frame);
    while alive {
        match raw.as_ref().and_then(|r| r.read_blocking()) {
            // 信号打断：先查 WINCH（可能伴随 resize），再回到阻塞读
            None => {
                if WINCH.swap(false, Ordering::Relaxed) {
                    let term = ffi::terminal_size();
                    alive = draw(&mut out, ctx, term, &frame);
                }
            }
            Some(Some(k)) => match k {
                b'q' | b'Q' | 0x1b | 0x03 => break,
                b'r' | b'R' => {
                    let term = ffi::terminal_size();
                    frame = next(term.map(|(_, cols)| cols));
                    alive = draw(&mut out, ctx, term, &frame);
                }
                _ => {}
            },
            // stdin EOF：终端已关闭
            Some(None) => break,
        }
    }
    let _ = out.write_all(b"\x1b[?1049l");
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::str_cells;

    fn ctx(no_panel: bool) -> Ctx {
        Ctx {
            big: false,
            canvas: None,
            center: false,
            no_panel,
            show_title: true,
            budget_enabled: true,
            module_names: sysinfo::resolve(None).unwrap(),
        }
    }

    fn frame() -> Frame {
        Frame {
            name: "pikachu".into(),
            shiny: false,
            ansi: "█\n██\n".into(),
        }
    }

    #[test]
    fn every_rendered_line_fits_terminal() {
        // 核心保证：任意列宽下渲染出的每个可见行 ≤ 终端宽（面板让位规则含内）
        for cols in [20usize, 25, 30, 40, 44, 52, 67, 80, 120] {
            let out = render(&ctx(false), Some((30, cols)), &frame());
            for line in out.lines() {
                assert!(
                    str_cells(line) <= cols,
                    "cols={cols} 超宽 {:#} 格: {line:?}",
                    str_cells(line)
                );
            }
        }
    }

    #[test]
    fn no_panel_and_degenerate_budget_only_sprite() {
        // 纯精灵与预算 0 让位：输出同为标题 + 精灵本体
        for c in [&ctx(true), &ctx(false)] {
            let out = render(c, Some((30, 20)), &frame());
            let lines: Vec<&str> = out.lines().collect();
            assert_eq!(lines[0], "pikachu");
            assert_eq!(lines.len(), 3);
        }
    }
}
