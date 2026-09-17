# pokefetch

Rust 写的宝可梦 fetch：终端里的精灵字符画 + 系统信息面板，也可作 fastfetch 的 logo 源。

素材来自 [pokemon-colorscripts](https://gitlab.com/phoneybadger/pokemon-colorscripts)（MIT），
已快照到 `assets/`，编译期把 ANSI 文本编码为调色板格子数组并逐精灵 zstd 压缩内嵌
（见 `build.rs` + `src/codec.rs`），运行时命中哪只解压哪只。打印语义与原始素材
逐格一致（build 期全量 round-trip 断言 + 5316 文件端到端比对验证）。

## 用法

```
pokefetch              # 精灵 + 系统面板（fetch 式，1/128 概率 shiny）
pokefetch --no-panel   # 纯精灵打印
pokefetch -n pikachu   # 指定（形态传全名如 charizard-mega-x）
pokefetch -n charizard -f mega-x   # 指定 + 形态
pokefetch -r 1-3       # 1~3 代随机
pokefetch --random-by-names pikachu,gengar  # 名单内随机
pokefetch -s -b --no-panel  # 闪光 + 大图纯精灵
pokefetch --watch      # 常驻重绘（见下文）
pokefetch --animated    # 动画播放（需动画数据，见下节）
pokefetch --animated --loops 3  # 播 3 轮后退出（省缺无限循环，任意键退出）
pokefetch -l           # 列出全部可用名字
```

1/128 的 shiny 概率只在随机模式（默认 / `-r` / `--random-by-names`）生效；
`-n` 指定名字时不掷点、恒为普通色，需要闪光显式加 `-s`。

精灵名字行默认隐藏，需要时加 `--title` 显示；`--raw` 恒不带名字行。

面板信息读自 /proc、/sys 与环境变量，零外部依赖，取不到的行自动跳过。
默认为精选模块集，`--modules os,gpu,memory,…` 可任意挑选
（可用：os host board bios kernel uptime packages shell de wm wmtheme theme icons
font cursor terminal gpu cpu memory swap disk localip localip-all battery load locale）。

## 接入 fastfetch

```
# 方式一：stdout 管道，零状态
fastfetch --data-raw "$(pokefetch -r --raw)"

# 方式二：缓存 + 随附 preset（每次 fetch 换精灵）
pokefetch -r --logo-cache && fastfetch --config fastfetch.jsonc
```

`--logo-cache` 把字符画写到 `~/.cache/pokefetch/logo.ans`（XDG_CACHE_HOME 感知），
preset 内的 logo 路径即指向它；`-o/--output <文件>` 可写入任意路径。

输出按画布右垫（small 默认 40 列，`--canvas <列宽>` 可调、0 关闭；large 不垫），
fastfetch 面板列位由此稳定；`--center` 可让精灵在画布内居中（默认左锚）。
随机时自动跳过当前终端放不下的精灵；
显式 `-n`/`-b` 完全按指定输出、不做干预。

## 动画播放

`--animated` 播放精灵的逐帧动画（Showdown 对战动画转译，帧率取素材原生 30–40ms）。
面板与名字行内容保持不变（播放时随精灵区域整行覆写，不闪烁）；
随机池、画布、`--center` 等行为与静态一致。播放中按任意键退出，
退出时的按键不会残留到 shell。

动画数据不进二进制。设置了环境变量 `POKEFETCH_ANIM` 时只使用它指向的
`anim.bin`；未设置才查找 `$XDG_DATA_HOME/pokefetch/anim.bin`
（默认 `~/.local/share/pokefetch/anim.bin`）。数据缺失、损坏或该精灵
没有动画帧时自动回退静态图并在 stderr 提示：

```
pokefetch --anim-pack <帧目录> -o anim.bin
```

帧目录布局为 `<帧目录>/<精灵名>/{regular,shiny}/NNNN.ans`（各变体目录内
附 `meta` 文件注明帧延时），`--anim-pack` 需传入包含全部精灵的上级目录。
打包完成后按上面两条路径之一放置即可（如 `-o ~/.local/share/pokefetch/anim.bin`）。

仅 stdout 直连终端时播放；`--raw`/`-o`/`--logo-cache`（fastfetch 对接路径）
与 `--animated` 互斥，恒为静态单帧输出。

## 常驻重绘

`--watch` 进入常驻模式：精灵 + 面板常驻当前窗口，终端尺寸变化（含平铺/
全屏切换引起的 resize）即整帧重排，随机池过滤与面板布局都按新宽度重算；
按 `r`/`R` 重掷一只（shiny 同分布），`q`/`Q`/`Esc`/`Ctrl-C` 退出，退出时还原终端
主屏。需 stdin/stdout 直连终端，与 `--raw`/`-o`/`--logo-cache`/`--animated`
互斥。

## 构建

```
cargo build --release
```

依赖 `ruzstd`（纯 Rust zstd 编解码）。

## 素材更新

`assets/` 是上游 pokemon-colorscripts 的快照，更新时从上游重新拷贝。
