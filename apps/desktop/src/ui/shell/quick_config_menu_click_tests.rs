//! `close_on_click_outside` 的点击消费回归：关弹层那一击不许穿透到下面的控件。

use super::{close_on_click_outside, config_menu_id};
use eframe::egui;
use egui::{Event, PointerButton, Pos2, RawInput, Rect, Vec2};

fn popup() -> Rect {
    Rect::from_min_size(Pos2::new(300.0, 100.0), Vec2::new(98.0, 182.0))
}

fn anchor() -> Rect {
    Rect::from_min_size(Pos2::new(300.0, 300.0), Vec2::new(30.0, 22.0))
}

/// 跑一帧：先开弹层，再投递一次点击，并在绘制期调用被测函数。
/// 返回 `(收起后是否仍开, 函数返回后 any_click 是否仍为真)`。
fn run_click(at: Pos2) -> (bool, bool) {
    let ctx = egui::Context::default();
    ctx.memory_mut(|m| m.open_popup(config_menu_id()));
    let mut open_after = false;
    let mut click_after = false;
    let _ = ctx.run(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
            events: vec![
                Event::PointerMoved(at),
                Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(ui.input(|i| i.pointer.any_click()), "测试帧必须有一次 click");
                close_on_click_outside(ui, popup(), anchor());
                open_after = ui.memory(|m| m.is_popup_open(config_menu_id()));
                click_after = ui.input(|i| i.pointer.any_click());
            });
        },
    );
    (open_after, click_after)
}

#[test]
fn an_outside_click_closes_the_popup_and_is_consumed() {
    let (open, click_left) = run_click(Pos2::new(600.0, 200.0));
    assert!(!open, "点外面必须收起弹层");
    assert!(!click_left, "这一击必须被消费，不能落到锚点下的控件上");
}

#[test]
fn clicks_on_the_popup_or_anchor_are_neither_closed_nor_consumed() {
    let (open, click_left) = run_click(Pos2::new(340.0, 120.0));
    assert!(open, "点在弹层里不该收起");
    assert!(click_left, "点在弹层里的 click 要留给菜单项");

    let (open, click_left) = run_click(Pos2::new(312.0, 310.0));
    assert!(open, "点在锚点上不该收起");
    assert!(click_left, "锚点自己的 click 要留给开关按钮");
}
