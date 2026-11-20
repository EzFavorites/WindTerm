//! 金样本回归集：固定字节流 → 网格快照 JSON。
//!
//! 这是整个一次性地基改造的「保险丝」。最容易静默改变 VT 行为，
//! 任何阶段只要 `cargo test golden` 与本目录下的基线 JSON 出现逐行差异，
//! 就说明语义被改动了，必须回滚，不得进入下一阶段。
//!
//! 用法：
//!   cargo test --manifest-path app/Cargo.toml golden       # 与基线比对
//!   GOLDEN_UPDATE=1 cargo test --manifest-path app/Cargo.toml golden -- --nocapture
//!                                                          # 重新生成基线（仅本文件允许）
//!
//! 快照只记录确定性字段：网格字符、非默认 cell 属性、光标、标题、备用屏、
//! scrollback 行数、设备应答、OSC 52 剪贴板状态。BEL 抑制属于宿主表现逻辑，
//! 不进入语义平面，因此快照中不存在任何时间戳字段。

use term_core::grid::{Attrs, Cell, Colour, CursorShape};
use term_core::vt::Terminal;
use serde_json::{json, Value};

const COLS: usize = 40;
const ROWS: usize = 10;
const SCROLLBACK: usize = 100;

fn cases() -> Vec<(&'static str, Vec<u8>)> {
    let mut v: Vec<(&'static str, Vec<u8>)> = vec![
        ("plain_text", b"hello\nworld\nthird\r\n".to_vec()),
        (
            "cursor_addressing",
            b"\x1b[3;7HX\x1b[2;2HY\x1b[1;1HZ\x1b[10;40HW".to_vec(),
        ),
        (
            "sgr_colours",
            b"\x1b[31mR\x1b[42mG\x1b[38;5;196mS\x1b[38;2;1;2;3mT\x1b[0mZ".to_vec(),
        ),
        (
            "text_attrs",
            b"\x1b[1mB\x1b[2mD\x1b[3mI\x1b[4mU\x1b[7mV\x1b[9mS\x1b[0mN".to_vec(),
        ),
        (
            "erase_and_clear",
            b"abcdef\x1b[2K\x1b[1;1HXY\x1b[2JQ\x1b[K".to_vec(),
        ),
        (
            "wide_cjk",
            "中文 abc\nconsecutive 中文文 tail\n"
                .as_bytes()
                .to_vec(),
        ),
        (
            "alt_screen",
            b"main\x1b[?1049h\x1b[2J\x1b[Halt\x1b[?1049l".to_vec(),
        ),
        (
            "device_queries",
            b"\x1b[6n\x1b[c\x1b[18t\x1b[5nx".to_vec(),
        ),
        (
            "osc_title",
            b"\x1b]0;my title\x07text\x1b]2;icon title\x07".to_vec(),
        ),
        (
            "osc52_clipboard",
            b"\x1b]52;c;aGVsbG8=\x07\x1b]52;c;?\x07".to_vec(),
        ),
        (
            "control_chars",
            b"a\rb\nc\td\x08e\x08f\n\rgh".to_vec(),
        ),
        (
            "save_restore_cursor",
            b"\x1b[2;3H\x1b7\x1b[8;9HS\x1b8R".to_vec(),
        ),
        (
            "scroll_region",
            b"top\x1b[2;5r\x1b[5;1Hl1\r\nl2\r\nl3\r\nl4\r\nl5".to_vec(),
        ),
    ];

    let mut scroll = String::new();
    for i in 1..=15 {
        scroll.push_str(&format!("line {i}\r\n"));
    }
    v.push(("scroll_off_top", scroll.into_bytes()));

    let mut wrap = "X".repeat(95);
    wrap.push_str("\r\nEND");
    v.push(("auto_wrap", wrap.into_bytes()));

    v
}

fn colour_json(c: Colour) -> Value {
    match c {
        Colour::Default => json!("d"),
        Colour::Indexed(i) => json!(["i", i]),
        Colour::Rgb(r, g, b) => json!(["r", r, g, b]),
    }
}

fn attrs_bits(a: Attrs) -> u8 {
    const FLAGS: [(u8, u8); 8] = [
        (1, Attrs::BOLD),
        (2, Attrs::DIM),
        (4, Attrs::ITALIC),
        (8, Attrs::UNDERLINE),
        (16, Attrs::BLINK),
        (32, Attrs::INVERSE),
        (64, Attrs::HIDDEN),
        (128, Attrs::STRIKE),
    ];
    let mut bits = 0u8;
    for (bit, flag) in FLAGS {
        if a.contains(flag) {
            bits |= bit;
        }
    }
    bits
}

fn cursor_shape_name(s: CursorShape) -> &'static str {
    match s {
        CursorShape::Block => "block",
        CursorShape::Underline => "underline",
        CursorShape::Beam => "beam",
        CursorShape::HollowBlock => "hollow_block",
    }
}

fn mark_cell(c: &Cell) -> Option<Value> {
    let fg = colour_json(c.fg);
    let bg = colour_json(c.bg);
    let bits = attrs_bits(c.attrs);
    if fg == json!("d") && bg == json!("d") && bits == 0 && !c.wide {
        return None;
    }
    Some(json!({"fg": fg, "bg": bg, "a": bits, "w": c.wide}))
}

fn snapshot(t: &Terminal, replies: &str) -> Value {
    let mut rows = Vec::with_capacity(t.grid.len());
    for (r, row) in t.grid.iter().enumerate() {
        let mut text = String::with_capacity(row.len());
        let mut marks = Vec::new();
        for (c, cell) in row.iter().enumerate() {
            text.push(cell.ch);
            if let Some(m) = mark_cell(cell) {
                marks.push(json!([c, m]));
            }
        }
        rows.push(json!({"r": r, "text": text, "marks": marks}));
    }

    json!({
        "cols": t.cols,
        "rows": t.rows,
        "cursor": [t.cursor_row, t.cursor_col],
        "cursor_visible": t.cursor_visible,
        "cursor_shape": cursor_shape_name(t.cursor_shape),
        "cursor_blink": t.cursor_blink,
        "title": t.title,
        "alt": t.in_alt_screen(),
        "scrollback": t.scrollback.len(),
        "replies": replies,
        "clipboard_request": t.clipboard_request,
        "clipboard_query": t.clipboard_query,
        "grid": rows,
    })
}

#[test]
fn golden_matches_baseline() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let update = std::env::var_os("GOLDEN_UPDATE").is_some();
    if update {
        std::fs::create_dir_all(&dir).unwrap();
    }

    let mut diffs: Vec<String> = Vec::new();
    for (name, bytes) in cases() {
        let mut t = Terminal::new(COLS, ROWS, SCROLLBACK);
        t.process(&bytes);
        let replies = String::from_utf8_lossy(&t.take_replies()).into_owned();
        let actual = serde_json::to_string_pretty(&snapshot(&t, &replies)).unwrap() + "\n";
        let path = dir.join(format!("{name}.json"));

        if update {
            std::fs::write(&path, &actual).unwrap();
            println!("wrote {}", path.display());
            continue;
        }

        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("缺少金样本 {}: {e}（需先 GOLDEN_UPDATE=1 生成）", path.display()));
        if expected != actual {
            let a = expected.lines().collect::<Vec<_>>();
            let b = actual.lines().collect::<Vec<_>>();
            let mut first = String::new();
            for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
                if x != y {
                    first = format!("  第 {} 行: 基线={:?} 实际={:?}", i + 1, x, y);
                    break;
                }
            }
            if first.is_empty() {
                first = format!("  行数不同: 基线 {} 行 / 实际 {} 行", a.len(), b.len());
            }
            diffs.push(format!("[{name}] {first}"));
        }
    }

    assert!(
        diffs.is_empty(),
        "金样本与基线不一致（{} 个 case）：\n{}\n语义发生变化，请回滚当前阶段，不得继续。",
        diffs.len(),
        diffs.join("\n")
    );
}
