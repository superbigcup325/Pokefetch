# pokefetch

Rust 写的宝可梦 fetch：随机或指定在终端打印一只宝可梦字符画。

素材来自 [pokemon-colorscripts](https://gitlab.com/phoneybadger/pokemon-colorscripts)（MIT），
已快照到 `assets/` 并在编译期内嵌（见 `build.rs`），运行零外部依赖。

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

## 素材更新

`assets/` 是上游 pokemon-colorscripts 的快照，更新时从上游重新拷贝。
