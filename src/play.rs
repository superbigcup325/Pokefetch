// 动画播放：条目获取（回退判定 + stderr 提示）与逐帧循环。
// 自 main.rs 拆出，main 回到纯编排。
//
// 生命周期契约（本模块的正确性所在，27deb83 踩过 termios 恢复坑）：
// - RawMode 守卫在 run() 函数内创建、随作用域 Drop 先 tcflush 清未读输入
//   再还原 termios（退出时的按键残留不得漏给 shell）——正常返回、
//   break 'rounds、panic unwind 三条退出路径全覆盖；
//   守卫不得提升到调用方作用域或任何更长生命周期，也不可 mem::forget。
// - stdout 锁同样函数内获取；Drop 顺序 LIFO：先还原 termios 再放锁。
// - 键盘监听借用守卫（read_key(&self)），循环结束监听即失效。

use std::io::Write;

use crate::anim;
use crate::canvas::compose;
use crate::ffi;

/// 动画路径：--animated 且 stdout 直连终端且数据命中；任一不满足回退静态图。
/// fastfetch 对接路径（--raw/-o/--logo-cache）被 clap 互斥挡住，恒为静态
pub(crate) fn prepare(animated: bool, shiny: bool, chosen: &str) -> Option<anim::Animation> {
    if !animated {
        return None;
    }
    if !ffi::stdout_is_tty() {
        eprintln!("pokefetch: stdout 不是终端，动画回退静态图");
        return None;
    }
    // 未找到/无法解析的消息由 AnimData::load 负责
    let data = anim::AnimData::load()?;
    let variant = if shiny { "shiny" } else { "regular" };
    match data.lookup(&format!("{variant}/{chosen}")) {
        Some(a) => Some(a),
        None => {
            eprintln!("pokefetch: 动画数据里没有 {chosen}，回退静态图");
            None
        }
    }
}

/// 逐帧播放：面板/名字行在块外保持静态，帧循环只覆写精灵+面板整行区
/// （光标上移到块首重打，行已按画布垫齐，无闪烁）。默认无限循环；
/// stdin 为 tty 且未指定 --loops 时进原始模式，任意键退出
pub(crate) fn run(
    animation: &anim::Animation,
    frames: &[String],
    panel: Option<&[String]>,
    sprite_w: usize,
    loops: Option<usize>,
) {
    let bodies: Vec<String> = frames
        .iter()
        .map(|f| match panel {
            Some(p) => compose(f, p.to_vec(), sprite_w, 3),
            None => f.clone(),
        })
        .collect();
    let rows = bodies[0].lines().count();
    let mut out = std::io::stdout().lock();
    let raw = if loops.is_none() && ffi::stdin_is_tty() {
        ffi::RawMode::enable()
    } else {
        None
    };
    'rounds: for round in 0..loops.unwrap_or(usize::MAX) {
        for (i, body) in bodies.iter().enumerate() {
            if round > 0 || i > 0 {
                // 光标回到本块首行（块尾以 \n 结束，正好落在块首行行首）
                write!(out, "\x1b[{rows}A").ok();
            }
            // stdout 不可写（终端关闭/EPIPE）即收场，继续循环只是空转
            if out.write_all(body.as_bytes()).is_err() || out.flush().is_err() {
                break 'rounds;
            }
            std::thread::sleep(animation.delay);
            // 被动观赏场景：任何按键都视为离场，无白名单（对照 --watch 的
            // q/Q/Esc/Ctrl-C + r/R，那边有重掷语义必须保留白名单）
            if raw.as_ref().and_then(|r| r.read_key()).is_some() {
                break 'rounds;
            }
        }
    }
}
