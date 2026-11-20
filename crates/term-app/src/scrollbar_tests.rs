//! P · 滚动条纯几何层单测（对应设计稿 §7.1）。
//!
//! 只验「数值 ↔ 数值」，不碰像素、不碰内核。所有断言里的几何都是纯数。

use crate::scrollbar::*;
use crate::viewport::Viewport;

// ── §7.1.1 history=0：滑块占满轨道、scroll_top_of 恒 0、无预览意义 ──

#[test]
fn no_history_means_full_track_and_no_scroll() {
    let track = Track {
        height: 1000.0,
        min_thumb: 12.0,
    };
    let thumb = thumb_of(track, 24, 0, 0);
    assert_eq!(thumb.top, 0.0);
    assert_eq!(thumb.height, 1000.0, "无历史时滑块占满整条轨道");
    // 任何滑块位置都反解为 0（贴底）。
    for top in [0.0, 250.0, 500.0, 999.0] {
        assert_eq!(scroll_top_of(track, 24, 0, top), 0);
    }
    // 预览在 history=0 时落在贴底（range 起点 0），且长度正确。
    let r = preview_range(0, 0.7, 10);
    assert_eq!(r.start().get(), 0);
    assert_eq!((r.end().get() - r.start().get()) as usize, 10);
}

#[test]
fn zero_visible_rows_also_fills_the_track() {
    let track = Track {
        height: 800.0,
        min_thumb: 12.0,
    };
    let thumb = thumb_of(track, 0, 100, 0);
    assert_eq!(thumb.height, 800.0);
    assert_eq!(thumb.top, 0.0);
    assert_eq!(scroll_top_of(track, 0, 100, 400.0), 0);
}

// ── §7.1.2 thumb_of / scroll_top_of 互为逆（多组 history/rows，允许 ±1 行） ──

#[test]
fn thumb_and_scroll_top_are_mutually_inverse() {
    let track = Track {
        height: 1022.0,
        min_thumb: 12.0,
    };
    let combos: &[(usize, usize)] = &[
        (24, 100),
        (24, 1000),
        (50, 7),
        (1, 999),
        (80, 200),
    ];
    for &(rows, history) in combos {
        for scroll_top in 0..=history as u64 {
            let thumb = thumb_of(track, rows, history, scroll_top);
            let back = scroll_top_of(track, rows, history, thumb.top);
            let diff = (back as i64 - scroll_top as i64).unsigned_abs();
            assert!(
                diff <= 1,
                "rows={rows} history={history} scroll_top={scroll_top} -> top={} -> back={back}",
                thumb.top
            );
        }
    }
}

#[test]
fn inverse_also_holds_across_thumb_positions() {
    let track = Track {
        height: 1022.0,
        min_thumb: 12.0,
    };
    let (rows, history) = (24, 1000);
    for i in 0..=100 {
        let top = (i as f64 / 100.0) * track.height;
        let st = scroll_top_of(track, rows, history, top);
        let thumb = thumb_of(track, rows, history, st);
        let back = scroll_top_of(track, rows, history, thumb.top);
        assert!(
            (back as i64 - st as i64).unsigned_abs() <= 1,
            "top={top} -> st={st} -> top2={} -> back={back}",
            thumb.top
        );
    }
}

// ── §7.1.3 最小高度下限 & 退化情形不产生负高度 ──

#[test]
fn min_thumb_floor_is_respected() {
    let track = Track {
        height: 1022.0,
        min_thumb: 40.0,
    };
    // 历史很短时滑块本应很小，被 min_thumb 托住。
    let thumb = thumb_of(track, 24, 5, 0);
    assert!(
        thumb.height >= 40.0 - 1e-9,
        "滑块高度被 min_thumb 托底：实际 {}",
        thumb.height
    );
    assert!(thumb.height > 0.0);
    assert!(thumb.top >= 0.0);
}

#[test]
fn thumb_height_is_never_negative_or_over_track() {
    let track = Track {
        height: 500.0,
        min_thumb: 12.0,
    };
    // min_thumb 超过 track 时仍被 track 拦死，高度不超过轨道。
    let over = Track {
        height: 500.0,
        min_thumb: 9999.0,
    };
    let t = thumb_of(over, 24, 100, 50);
    assert!(t.height <= 500.0 + 1e-9);
    assert!(t.height >= 0.0);
    // 常规组合：高度恒定在 (0, track]。
    for (rows, history, st) in [(1usize, 1usize, 0u64), (24, 100, 50), (80, 200, 199)] {
        let t = thumb_of(track, rows, history, st);
        assert!(t.height > 0.0 && t.height <= 500.0 + 1e-9, "{t:?}");
    }
}

// ── §7.1.4 preview_range 在 ratio 0/0.5/1 分别落历史顶端/中部/贴底，len==preview_rows ──

#[test]
fn preview_range_maps_ratio_to_history_positions() {
    let history = 1000usize;
    let preview_rows = 24usize;

    let top = preview_range(history, 0.0, preview_rows);
    assert_eq!(top.start().get(), -(history as i64)); // 最老的行
    assert_eq!((top.end().get() - top.start().get()) as usize, preview_rows);

    let mid = preview_range(history, 0.5, preview_rows);
    // ratio=0.5 -> scroll_top≈500 -> scroll≈500 -> 起点 ≈ -500
    assert!(
        mid.start().get().abs() <= 501 && mid.start().get() <= 0,
        "ratio=0.5 起点约 -500，实际 {}",
        mid.start().get()
    );
    assert_eq!((mid.end().get() - mid.start().get()) as usize, preview_rows);

    let bottom = preview_range(history, 1.0, preview_rows);
    assert_eq!(bottom.start().get(), 0, "ratio=1 贴底，起点 0");
    assert_eq!((bottom.end().get() - bottom.start().get()) as usize, preview_rows);
}

#[test]
fn preview_range_is_clamped_to_unit_ratio() {
    let r = preview_range(500, 5.0, 10);
    assert_eq!(r.start().get(), 0, "越界 ratio 钳到 1（贴底）");
    let r2 = preview_range(500, -3.0, 10);
    assert_eq!(r2.start().get(), -500, "负 ratio 钳到 0（历史顶端）");
}

// ── §7.1.5 preview_box_top 钳制：复现官方 GIF frame 30 ──

#[test]
fn preview_box_top_reproduces_frame_30() {
    // 设计稿 §2.6：hover_y=426, box_h=576, content_top=195, content_h=1022 -> 195
    let top = preview_box_top(195.0, 1022.0, 576.0, 426.0);
    assert_eq!(top as i64, 195, "被顶部钳住，复现 frame 30 实测 196（±1 取整）");
}

#[test]
fn preview_box_top_clamps_at_both_ends() {
    // 鼠标太靠下：框被底部钳住。
    let max_top = 195.0 + 1022.0 - 576.0; // = 641
    let low = preview_box_top(195.0, 1022.0, 576.0, 1200.0);
    assert!((low - max_top).abs() < 1e-6, "low={low} max_top={max_top}");
    // 鼠标居中：框中心跟随 hover。
    let mid = preview_box_top(195.0, 1022.0, 576.0, 600.0);
    assert!((mid - (600.0 - 288.0)).abs() < 1e-6, "mid={mid}");
    // 内容区比框还矮：max_top 退化到 content_top，框不出内容区。
    let tiny = preview_box_top(0.0, 100.0, 576.0, 50.0);
    assert_eq!(tiny, 0.0);
}

// ── §7.1.6 preview_range 与「同 scroll_top 下真实 Viewport::rows()」一致 ──

#[test]
fn preview_range_matches_a_real_viewport_at_the_same_scroll_top() {
    let history = 800usize;
    let preview_rows = 30usize;
    for ratio in [0.0f64, 0.25, 0.5, 0.75, 1.0] {
        let st = (ratio * history as f64).round() as u64;
        let from_fn = preview_range(history, ratio, preview_rows);

        let mut vp = Viewport::new();
        vp.apply_scroll_top(st, history as u64);
        let from_vp = vp.rows(preview_rows);

        assert_eq!(from_fn, from_vp, "ratio={ratio} scroll_top={st}");
    }
}

#[test]
fn preview_range_len_is_always_preview_rows() {
    for history in [0usize, 1, 7, 50, 1000] {
        for ratio in [0.0f64, 0.3, 0.6, 1.0] {
            let r = preview_range(history, ratio, 24);
            assert_eq!((r.end().get() - r.start().get()) as usize, 24);
        }
    }
}

#[test]
fn thumb_top_is_proportional_to_scroll_top() {
    let track = Track {
        height: 1000.0,
        min_thumb: 12.0,
    };
    let (rows, history) = (24, 1000);
    let t0 = thumb_of(track, rows, history, 0).top;
    let t1 = thumb_of(track, rows, history, history as u64).top;
    // 贴底时滑块靠近轨道底（top 接近 travel），历史顶端时 top=0。
    assert_eq!(t0, 0.0);
    let travel = track.height - thumb_of(track, rows, history, 0).height;
    assert!((t1 - travel).abs() < 1e-6, "t1={t1} travel={travel}");
    // 单调性：scroll_top 越大，滑块越靠下。
    let mid = thumb_of(track, rows, history, (history / 2) as u64).top;
    assert!(t0 < mid && mid < t1, "t0={t0} mid={mid} t1={t1}");
}
