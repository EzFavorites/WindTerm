//! `close_on_click_outside` 的回归：关弹层那一击只删 Primary 的 release，
//! **不清本帧已算好的 `clicked()` 边沿**，所以下游控件必须查 `click_consumed_at`
//! 跳过；hover / release / 其它按钮仍照常工作。

use super::{click_consumed_at, close_on_click_outside, config_menu_id};
use crate::ui::shell::quick_command_bar::click_not_consumed;
// 终端侧两个分支的真实闸门：与 pane.rs 调用的是同一对函数。
use crate::ui::terminal::pane_click_gate::{fold_click_allowed, mouse_encoding_allowed};
use eframe::egui;
use egui::{Event, PointerButton, Pos2, RawInput, Rect, Vec2};

fn popup() -> Rect {
    Rect::from_min_size(Pos2::new(300.0, 100.0), Vec2::new(98.0, 182.0))
}

fn anchor() -> Rect {
    Rect::from_min_size(Pos2::new(300.0, 300.0), Vec2::new(30.0, 22.0))
}

struct Frame {
    open: bool,
    hover_after: Option<Pos2>,
    any_released: bool,
    /// 未经闸门的原边沿：证明「只删 release」并没有清掉本帧 click。
    raw_clicked: bool,
    /// 生产控件走的判据：`clicked() && click_not_consumed()`。
    later_clicked: bool,
    consumed: bool,
    /// 终端折叠列 / xterm 鼠标报告两个分支的同一帧判据。
    fold_allowed: bool,
    mouse_allowed: bool,
}

/// 跑两帧：第一帧预热（egui 的命中测试用上一帧 widget rect），第二帧投递点击。
/// `close_on_click_outside` 在「锚点下控件」之前调用，与生产绘制顺序一致。
fn run_click(at: Pos2) -> Frame {
    let ctx = egui::Context::default();
    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
    let later_rect = Rect::from_center_size(at, Vec2::splat(60.0));
    let mut out = Frame {
        open: true,
        hover_after: None,
        any_released: false,
        raw_clicked: false,
        later_clicked: false,
        consumed: false,
        fold_allowed: false,
        mouse_allowed: false,
    };

    let frames = [
        vec![Event::PointerMoved(at)],
        vec![
            Event::PointerMoved(at),
            Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() },
            Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() },
        ],
    ];
    for (pass, events) in frames.into_iter().enumerate() {
        let click_pass = pass == 1;
        let _ = ctx.run(
            RawInput { screen_rect: Some(screen), events, ..Default::default() },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.memory_mut(|m| m.open_popup(config_menu_id()));
                    close_on_click_outside(ui, popup(), anchor());
                    let later = ui.interact(later_rect, egui::Id::new("later_widget"), egui::Sense::click());
                    if click_pass {
                        out.open = ui.memory(|m| m.is_popup_open(config_menu_id()));
                        out.hover_after = ui.input(|i| i.pointer.hover_pos());
                        out.any_released = ui.input(|i| i.pointer.any_released());
                        out.raw_clicked = later.clicked();
                        out.later_clicked = out.raw_clicked && click_not_consumed(ui);
                        out.consumed = click_consumed_at(ui, at);
                        out.fold_allowed = fold_click_allowed(ui, out.raw_clicked);
                        out.mouse_allowed = mouse_encoding_allowed(ui);
                    }
                });
            },
        );
    }
    out
}

#[test]
fn an_outside_click_closes_the_popup_and_records_the_consumed_position() {
    let at = Pos2::new(600.0, 200.0);
    let frame = run_click(at);
    assert!(!frame.open, "点外面必须收起弹层");
    assert!(frame.consumed, "这一击的位置要记下来，供锚点下控件跳过");
}

#[test]
fn the_consuming_click_keeps_hover_and_release_but_blocks_the_later_control() {
    let frame = run_click(Pos2::new(600.0, 200.0));
    assert_eq!(frame.hover_after, Some(Pos2::new(600.0, 200.0)), "hover 不许被清空");
    assert!(frame.any_released, "同帧其它按钮的 release 边沿不许被抹掉");
    assert!(frame.raw_clicked, "只删 raw release 不会清掉帧首已算好的 click 边沿");
    assert!(!frame.later_clicked, "点外收起后，下方控件的接线判据必须为 false");
}

#[test]
fn clicks_on_the_popup_or_anchor_are_neither_closed_nor_consumed() {
    let frame = run_click(Pos2::new(340.0, 120.0));
    assert!(frame.open, "点在弹层里不该收起");
    assert!(!frame.consumed, "弹层里的点击要原样留给菜单项");
    assert!(frame.later_clicked);

    let frame = run_click(Pos2::new(312.0, 310.0));
    assert!(frame.open, "点在锚点上不该收起");
    assert!(!frame.consumed, "锚点自己的点击要原样留给开关按钮");
    assert!(frame.later_clicked);
}

/// 接线级：收起弹层那一击同帧**不触发折叠、不编码鼠标**。
/// 判据走的是 `pane.rs` 两个分支真正调用的 `pane_click_gate` 函数，而不是复制品。
#[test]
fn the_dismissing_click_also_blocks_terminal_fold_and_mouse_encoding() {
    let outside = run_click(Pos2::new(600.0, 200.0));
    assert!(outside.raw_clicked, "帧首边沿仍在——证明不是靠清 `clicked()` 实现的");
    assert!(!outside.fold_allowed, "点外收起后同帧不许翻折叠列");
    assert!(!outside.mouse_allowed, "点外收起后同帧不许往会话编码鼠标");

    // 对照组：点在弹层里不消费，终端两个分支照常放行。
    let inside = run_click(Pos2::new(340.0, 120.0));
    assert!(inside.fold_allowed, "弹层内的点击不该挡住终端折叠");
    assert!(inside.mouse_allowed, "弹层内的点击不该挡住鼠标报告");
}
