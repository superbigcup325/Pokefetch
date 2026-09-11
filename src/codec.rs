// 编码核心：ANSI truecolor 字符画 <-> 调色板格子数组
// build.rs（编码 + round-trip 断言）与运行时（解码渲染）共用，只依赖 std。
// 本文件同时被两处引用，未用到的函数按侧不同属正常，整体放行 dead_code。
#![allow(dead_code)]
//
// 素材格式是实测的封闭集：
// - 字形仅 4 种：空格 / ▀ / ▄ / █（2bit 语义，存 u8）
// - SGR 仅三种形态：`38;2;r;g;b`、`48;2;r;g;b`、`0m`，无其他转义
// 任何超出封闭集的输入一律 panic，拒绝静默失真。

pub type Color = (u8, u8, u8);

pub const GLYPHS: &[char] = &[' ', '▀', '▄', '█'];

/// 规范形：调色板 1-based（0 = 透明），cells 行主序、补齐到 cols
pub struct Sprite {
    pub rows: usize,
    pub cols: usize,
    pub pal: Vec<Color>,
    pub cells: Vec<(u16, u16, u8)>,
}

/// 语义形态：格子直接用 RGB 三元组，比较时与调色板顺序无关
type SemanticCell = (Option<Color>, Option<Color>, char);

fn semantic(sprite: &Sprite) -> Vec<SemanticCell> {
    sprite
        .cells
        .iter()
        .map(|&(f, b, g)| {
            (
                (f != 0).then(|| sprite.pal[(f - 1) as usize]),
                (b != 0).then(|| sprite.pal[(b - 1) as usize]),
                GLYPHS[g as usize],
            )
        })
        .collect()
}

pub fn semantic_eq(a: &Sprite, b: &Sprite) -> bool {
    a.rows == b.rows && a.cols == b.cols && semantic(a) == semantic(b)
}

/// 解析原始 ANSI 文本；非法输入 panic（msg 由调用方附上文件名）
pub fn parse_ansi(text: &str) -> Sprite {
    type RawCell = (Option<Color>, Option<Color>, u8);
    let mut fg = None::<Color>;
    let mut bg = None::<Color>;
    let mut rows: Vec<Vec<RawCell>> = Vec::new();
    let mut row: Vec<RawCell> = Vec::new();

    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                rows.push(std::mem::take(&mut row));
                i += 1;
            }
            0x1b => {
                // 严格匹配 ESC [ params m
                let end = text[i..]
                    .find('m')
                    .unwrap_or_else(|| panic!("未闭合的转义序列"));
                let params = &text[i + 2..i + end];
                assert_eq!(bytes[i + 1], b'[', "非 SGR 转义序列");
                assert!(
                    params.bytes().all(|c| c.is_ascii_digit() || c == b';'),
                    "SGR 参数含非法字符"
                );
                if params == "0" {
                    fg = None;
                    bg = None;
                } else {
                    let p: Vec<&str> = params.split(';').collect();
                    match (p[0], p.len()) {
                        ("38", 5) => fg = Some(parse_rgb(&p)),
                        ("48", 5) => bg = Some(parse_rgb(&p)),
                        _ => panic!("不支持的 SGR 形态: {params}"),
                    }
                }
                i += end + 1;
            }
            _ => {
                let ch = text[i..].chars().next().unwrap();
                let g =
                    GLYPHS.iter().position(|&c| c == ch).unwrap_or_else(|| {
                        panic!("字形封闭集之外的字符: {ch:?} (U+{:04X})", ch as u32)
                    }) as u8;
                row.push((fg, bg, g));
                i += ch.len_utf8();
            }
        }
    }
    rows.push(row);

    // 文件以 \n 结尾时最后一段是空行，丢弃
    if rows.last().is_some_and(|r| r.is_empty()) {
        rows.pop();
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);

    let mut pal: Vec<Color> = Vec::new();
    let mut rev: std::collections::HashMap<Color, u16> = std::collections::HashMap::new();
    let idx = |c: Option<Color>,
               pal: &mut Vec<Color>,
               rev: &mut std::collections::HashMap<Color, u16>|
     -> u16 {
        match c {
            None => 0,
            Some(c) => *rev.entry(c).or_insert_with(|| {
                pal.push(c);
                pal.len() as u16
            }),
        }
    };
    let mut cells = Vec::with_capacity(rows.len() * cols);
    for r in &rows {
        for (f, b, g) in r {
            cells.push((idx(*f, &mut pal, &mut rev), idx(*b, &mut pal, &mut rev), *g));
        }
        cells.extend(std::iter::repeat_n((0u16, 0u16, 0u8), cols - r.len()));
    }

    Sprite {
        rows: rows.len(),
        cols,
        pal,
        cells,
    }
}

fn parse_rgb(p: &[&str]) -> Color {
    let v = |s: &str| -> u8 { s.parse().unwrap_or_else(|_| panic!("RGB 分量非法: {s}")) };
    (v(p[2]), v(p[3]), v(p[4]))
}

/// 编码为二进制：
/// rows u8 | cols u8 | wide u8 | n_pal u16le | pal n×3B | cells rows×cols×(3B | 5B)
/// wide = 调色板超 255 色时 cells 用 u16le 索引
pub fn encode(s: &Sprite) -> Vec<u8> {
    assert!(s.rows <= u8::MAX as usize && s.cols <= u8::MAX as usize);
    assert!(s.pal.len() < u16::MAX as usize);
    let wide = s.pal.len() > 255;
    let mut out =
        Vec::with_capacity(5 + s.pal.len() * 3 + s.cells.len() * if wide { 5 } else { 3 });
    out.push(s.rows as u8);
    out.push(s.cols as u8);
    out.push(wide as u8);
    out.extend_from_slice(&(s.pal.len() as u16).to_le_bytes());
    for (r, g, b) in &s.pal {
        out.extend_from_slice(&[*r, *g, *b]);
    }
    for &(f, b, g) in &s.cells {
        if wide {
            out.extend_from_slice(&f.to_le_bytes());
            out.extend_from_slice(&b.to_le_bytes());
        } else {
            out.push(f as u8);
            out.push(b as u8);
        }
        out.push(g);
    }
    out
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> &'a [u8] {
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        s
    }
    fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }
    fn u16le(&mut self) -> u16 {
        u16::from_le_bytes(self.take(2).try_into().unwrap())
    }
}

/// 编码帧头部推出该帧的编码字节长度（动图多帧串联时切帧用）
pub fn encoded_size(data: &[u8]) -> usize {
    let rows = data[0] as usize;
    let cols = data[1] as usize;
    let wide = data[2] != 0;
    let n_pal = u16::from_le_bytes(data[3..5].try_into().unwrap()) as usize;
    5 + n_pal * 3 + rows * cols * if wide { 5 } else { 3 }
}

/// 从编码字节直接渲染 ANSI 到 out（运行时路径，零中间结构）
pub fn decode_and_render(data: &[u8], out: &mut String) {
    let mut r = Reader { data, pos: 0 };
    let rows = r.u8() as usize;
    let cols = r.u8() as usize;
    let wide = r.u8() != 0;
    let n_pal = r.u16le() as usize;
    let pal: Vec<Color> = r
        .take(n_pal * 3)
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| (c[0], c[1], c[2]))
        .collect();

    let mut curf = 0usize;
    let mut curb = 0usize;
    let set_fg = |out: &mut String, i: usize| {
        let (r, g, b) = pal[i - 1];
        out.push_str(&format!("\x1b[38;2;{r};{g};{b}m"));
    };
    let set_bg = |out: &mut String, i: usize| {
        let (r, g, b) = pal[i - 1];
        out.push_str(&format!("\x1b[48;2;{r};{g};{b}m"));
    };
    let mut cells = Reader {
        data,
        pos: 5 + n_pal * 3,
    };
    for _row in 0..rows {
        for _col in 0..cols {
            let (f, b, g) = if wide {
                (
                    cells.u16le() as usize,
                    cells.u16le() as usize,
                    cells.u8() as usize,
                )
            } else {
                (
                    cells.u8() as usize,
                    cells.u8() as usize,
                    cells.u8() as usize,
                )
            };
            // 透明/换色语义与 parse 侧对称：只在状态变化处发 SGR；
            // 清掉仅剩的一路颜色只能整体 reset
            if f == 0 && b == 0 {
                if curf != 0 || curb != 0 {
                    out.push_str("\x1b[0m");
                    curf = 0;
                    curb = 0;
                }
            } else {
                if (curf != 0 && f == 0) || (curb != 0 && b == 0) {
                    out.push_str("\x1b[0m");
                    curf = 0;
                    curb = 0;
                }
                if f != curf {
                    set_fg(out, f);
                    curf = f;
                }
                if b != 0 && b != curb {
                    set_bg(out, b);
                    curb = b;
                }
            }
            out.push(GLYPHS[g]);
        }
        if curf != 0 || curb != 0 {
            out.push_str("\x1b[0m");
            curf = 0;
            curb = 0;
        }
        out.push('\n'); // 素材文件均以 \n 结尾，与原直出行为对齐
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: &str = "\x1b[38;2;255;0;0m█\x1b[0m";
    const BG: &str = "\x1b[48;2;0;0;255m \x1b[0m";

    fn roundtrip(text: &str) -> String {
        let s = parse_ansi(text);
        let mut out = String::new();
        decode_and_render(&encode(&s), &mut out);
        out
    }

    #[test]
    fn roundtrip_semantic_eq() {
        let cases = [
            RED,
            BG,
            &format!("{RED}{BG}█"),
            "  █▀▄  ",
            &format!("{RED}█  {BG}  █\x1b[0m{RED}█"),
        ];
        for c in cases {
            let s1 = parse_ansi(c);
            let s2 = parse_ansi(&roundtrip(c));
            assert!(semantic_eq(&s1, &s2), "roundtrip 失败: {c:?}");
        }
    }

    #[test]
    fn trailing_newline_dropped() {
        let s = parse_ansi("█\n█\n");
        assert_eq!(s.rows, 2);
    }

    #[test]
    fn ragged_rows_padded() {
        let s = parse_ansi("█\n██");
        assert_eq!((s.rows, s.cols), (2, 2));
        assert_eq!(s.cells[1], (0, 0, 0)); // 补齐透明格
    }

    #[test]
    fn wide_palette_encoding() {
        // 300 种颜色排成 16 列网格（rows/cols 都在 u8 内），触发 u16 索引路径
        let mut text = String::new();
        for i in 0..304u16 {
            if i > 0 && i % 16 == 0 {
                text.push('\n');
            }
            text.push_str(&format!("\x1b[38;2;{};{};0m█\x1b[0m", i & 0xff, i >> 8));
        }
        let s = parse_ansi(&text);
        assert_eq!(s.pal.len(), 304);
        let s2 = parse_ansi(&roundtrip(&text));
        assert!(semantic_eq(&s, &s2));
    }

    #[test]
    #[should_panic(expected = "字形封闭集之外")]
    fn unknown_glyph_panics() {
        parse_ansi("█x");
    }

    #[test]
    #[should_panic(expected = "不支持的 SGR")]
    fn unknown_sgr_panics() {
        parse_ansi("\x1b[1m█");
    }
}
