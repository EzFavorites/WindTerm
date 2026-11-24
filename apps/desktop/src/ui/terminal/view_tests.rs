//! `terminal/view.rs` 单测（拆出以守 500 行门禁）。

use super::*;

use super::*;

    fn row(text: &str) -> Vec<Cell> {
        let mut v: Vec<Cell> = text.chars().map(|ch| Cell { ch, ..Cell::default() }).collect();
        v.resize(80, Cell::default());
        v
    }

    /// 「段驱动」零像素变化的身份性前提。
    ///
    /// `paint_rows` 现在按段画，`y` 与缓存键都用**段序号**。它之所以与改造前
    /// 逐字节相同，靠的是一条外部事实：内核本来就在**同一个列宽**上硬折过行，
    /// 所以每条 `LineSlice` 的显示宽度 ≤ `image.cols`，按 `image.cols` 折出来
    /// 恰好 1 段、且覆盖整行。
    ///
    /// 这条测试就是那个前提的可证伪形式：哪天内核改成不在同宽折行（或投影给了
    /// 比网格更宽的行），它先红——而不是等到人眼在某张截图上看出「行距变了」
    /// 再去反推坐标系。
    #[test]
    fn every_grid_row_wraps_into_exactly_one_segment() {
        let cols = 12usize;
        // 三行覆盖三种真实形状：全空白（内核对每一行都补齐到 cols；这些空格是
        // 真实的格子，不许当尾随空白裁掉）、满格 ASCII、宽字符。
        let blank = vec![Cell::default(); cols];
        let mut full = blank.clone();
        // 恰好 12 个字符 = 满格。
        for (i, ch) in "hello world!".chars().enumerate() {
            full[i].ch = ch;
        }
        // 宽字符占**两**格（字形格 + 占位格）：字形数与显示列数不相等，
        // 「逐 cell 一项」的宽度表在这一行会多算一列。
        let mut cjk = blank.clone();
        cjk[0] = Cell { ch: '中', wide: true, ..Cell::default() };
        cjk[2] = Cell { ch: 'a', ..Cell::default() };
        let lines: Vec<term_core::grid::LineSlice> = [blank, full, cjk]
            .into_iter()
            .enumerate()
            .map(|(i, cells)| term_core::grid::LineSlice { row: i as i64, cells })
            .collect();
        let image = ScreenImage {
            cols,
            version: 0,
            range: 0..lines.len() as i64,
            lines,
            cursor: None,
            history: 0,
        };

        // 与 `pane_body::paint_body` 完全相同的一条数据流。
        let widths: Vec<Vec<u8>> = image
            .lines
            .iter()
            .map(|l| term_app::wrap::glyph_widths(&l.cells))
            .collect();
        let segments = term_app::wrap::wrap_lines(&widths, image.cols);

        assert_eq!(
            segments.len(),
            image.lines.len(),
            "每条 LineSlice 必须恰好折出 1 段，否则 y 坐标与缓存键不再与改造前等价"
        );
        for (seg, line) in segments.iter().zip(&image.lines) {
            let want = term_app::wrap::VisualRow {
                line: seg.line,
                start_col: 0,
                width: line.cells.len(),
                soft_continued: false,
                hard_continued: false,
            };
            assert_eq!(*seg, want, "第 {} 行折出了不止一段或不是整行", line.row);
        }
    }

    #[test]
    fn blank_row_has_no_spans() {
        assert!(build_spans(&row("")).is_empty());
    }

    #[test]
    fn plain_text_merges_into_one_span() {
        let spans = build_spans(&row("hello"));
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "hello");
        assert_eq!(spans[0].col, 0);
    }

    #[test]
    fn a_space_breaks_the_span() {
        let spans = build_spans(&row("hello world"));
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "hello");
        assert_eq!(spans[1].col, 6);
        assert_eq!(spans[1].text, "world");
    }

    #[test]
    fn a_colour_change_breaks_the_span() {
        let mut cells = row("abcd");
        cells[2].fg = Colour::Indexed(1);
        let spans = build_spans(&cells);
        // 默认色 "ab" / 变色 "c" / 恢复默认 "d" → 3 段
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].text, "ab");
        assert_eq!(spans[1].text, "c");
        assert_eq!(spans[2].text, "d");
    }

    #[test]
    fn wide_glyph_is_its_own_span_and_skips_placeholder() {
        let mut cells = row("中 x");
        cells[0].wide = true;
        // cells[1] 是宽字符占位（空格），cells[2] = 'x'
        let spans = build_spans(&cells);
        assert_eq!(spans.len(), 2, "{spans:?}");
        assert_eq!(spans[0].text, "中");
        assert_eq!(spans[1].text, "x");
        assert_eq!(spans[1].col, 2);
    }

    #[test]
    fn bg_runs_merge_consecutive_cells() {
        let mut cells = row("abcd");
        cells[1].bg = Colour::Indexed(2);
        cells[2].bg = Colour::Indexed(2);
        let runs = build_bg_runs(&cells);
        assert_eq!(runs.len(), 1, "连续同色应合成一个矩形");
        assert_eq!(runs[0], BgRun { col: 1, end_col: 3, bg: Colour::Indexed(2) });
    }

    #[test]
    fn bg_runs_split_on_colour_change() {
        let mut cells = row("ab");
        cells[0].bg = Colour::Indexed(1);
        cells[1].bg = Colour::Indexed(2);
        assert_eq!(build_bg_runs(&cells).len(), 2);
    }

    #[test]
    fn cache_hit_does_not_rebuild() {
        let mut cache = LineCache::default();
        let cells = row("hello world");
        let first = cache.line(0, &cells).clone();
        let second = cache.line(0, &cells).clone();
        assert_eq!(first.key, second.key);
        assert_eq!(first.spans, second.spans);
    }

    #[test]
    fn cache_invalidates_when_the_row_changes() {
        let mut cache = LineCache::default();
        let a = cache.line(0, &row("hello")).clone();
        let b = cache.line(0, &row("world")).clone();
        assert_ne!(a.key, b.key);
        assert_eq!(b.spans[0].text, "world");
    }

    #[test]
    fn typical_grid_needs_far_fewer_draw_calls_than_cells() {
        // 24 行 × 80 列，每行 "hello world " 重复：1920 个 cell。
        let mut cache = LineCache::default();
        let mut calls = 0usize;
        for r in 0..24 {
            let cells = row(&"hello world ".repeat(6));
            let line = cache.line(r, &cells);
            calls += line.spans.len() + line.bg_runs.len();
        }
        // 12 个单词/行 × 24 行 = 288 次文字调用，背景 0 次。
        assert_eq!(calls, 288);
        assert!(calls < 1920 / 4, "合批后调用数应显著低于逐 cell 绘制");
    }

    #[test]
    fn alternate_row_background_shades_odd_absolute_rows_only() {
        // 第 0 行保持原底色（每屏最上面那一行），从第 1 行起隔行着色。
        assert!(!shades_row(0), "顶端第一行不该变色");
        assert!(shades_row(1));
        assert!(!shades_row(2));
        assert!(shades_row(3));
        // 回看到历史起点之前时行号可能是负的：仍然只按奇偶分，不会因为符号翻掉
        // （`-1 % 2 == -1`，所以这里断言的是「非零即着色」，不是「等于 1」）。
        assert!(shades_row(-1));
        assert!(!shades_row(-2));
        // 相邻两行一个着色一个不着色——这正是「隔行」的定义，写死成常量就会红。
        for r in -4..4 {
            assert_ne!(shades_row(r), shades_row(r + 1), "第 {r} 行与下一行同色");
        }
    }

    #[test]
    fn the_alternate_row_tint_is_a_faint_alpha_of_the_foreground() {
        // 断言看的是 **premultiplied 的原始字节**：epaint 的混合是
        // `src.rgb + dst.rgb × (1 − a)`，所以真正加到画面上的是 rgb 这几个字节
        // （4% ≈ 10/255），不是 `to_srgba_unmultiplied` 换算回来的值——那个函数
        // 走的是线性空间，与绘制时的 sRGB 混合不是同一套口径。
        let tint = alternate_row_tint(Color32::WHITE);
        assert_eq!(tint.a(), 10, "4% 的 alpha ≈ 10/255");
        assert_eq!(
            (tint.r(), tint.g(), tint.b()),
            (10, 10, 10),
            "白前景叠出来的是「提亮 4%」"
        );
        // 浅底主题的前景是近黑：同一个 alpha、方向相反（压暗 4%）。这就是为什么
        // 用前景色而不是写死一个灰——写死的灰在另一种主题上是「一块色条」。
        let dark = alternate_row_tint(Color32::BLACK);
        assert_eq!(dark.a(), tint.a(), "浓度与主题无关");
        assert_eq!((dark.r(), dark.g(), dark.b()), (0, 0, 0), "纯黑只压 alpha");
        // premultiplied 的合法条件：每个通道都不超过 alpha。
        for c in [tint, dark] {
            assert!(c.r() <= c.a() && c.g() <= c.a() && c.b() <= c.a());
        }
    }
