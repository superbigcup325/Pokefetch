# pokefetch

Rust 写的宝可梦 fetch：随机或指定在终端打印一只宝可梦字符画。

素材来自 [pokemon-colorscripts](https://gitlab.com/phoneybadger/pokemon-colorscripts)（MIT），
已快照到 `assets/`，编译期把 ANSI 文本编码为调色板格子数组并逐精灵 zstd 压缩内嵌
（见 `build.rs` + `src/codec.rs`），运行时命中哪只解压哪只。打印语义与原始素材
逐格一致（build 期全量 round-trip 断言 + 5316 文件端到端比对验证）。

## 用法

```
pokefetch              # 随机一只（1/128 概率 shiny）
pokefetch -n pikachu   # 指定
pokefetch -r 1-3       # 1~3 代随机
pokefetch -s -b        # 闪光 + 大图
```

## 构建

```
cargo build --release
```

依赖 `ruzstd`（纯 Rust zstd 编解码）。

## 素材更新

`assets/` 是上游 pokemon-colorscripts 的快照，更新时从上游重新拷贝。
