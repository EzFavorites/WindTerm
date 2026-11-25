//! 专注模式的地址栏门控判据（由 `#[path]` 挂回 `address/mod.rs`）。
//!
//! 审查补充：专注模式只留菜单栏，但 Alt+D 仍能经 dispatch 把 `AddressState`
//! 置开，调用点也没有 `!focus` 门——键盘与画面都会冒出一个本该被压掉的
//! 输入框。这里从三个入口（dispatch / open / draw）逐条钉死。

use super::*;
use crate::ui::lifecycle_test_support::window_with;
use term_app::vocab::Action;

fn window() -> crate::ui::TerminalWindow {
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    window_with(pool, &ctx, Vec::new())
}

/// Alt+D 走 dispatch：专注模式下不许把 AddressState 置开；退出专注后照常开。
#[test]
fn focus_mode_blocks_the_address_bar_action() {
    let mut win = window();
    win.view.focus_mode = true;
    win.dispatch_action(Action::AddressBar);
    assert!(!win.address.state.open, "专注模式里 Alt+D 把地址栏置开了");

    win.view.focus_mode = false;
    win.dispatch_action(Action::AddressBar);
    assert!(win.address.state.open, "退出专注后地址栏必须照常可开");
}

/// 工具栏那一下绕过 dispatch 直接调 open：统一门也要拦住。
#[test]
fn the_open_helper_refuses_while_focused() {
    let mut win = window();
    win.view.focus_mode = true;
    win.open_address_bar("ssh://host", None);
    assert!(!win.address.state.open);
    assert!(win.address.anchor.is_none(), "连锚点都不该被改写");

    win.view.focus_mode = false;
    win.open_address_bar("ssh://host", None);
    assert!(win.address.state.open);
}

/// 即使状态被别处播种成 open，专注模式下也不画任何形状。
#[test]
fn a_focused_window_paints_no_address_bar() {
    let ctx = egui::Context::default();
    let mut win = window();
    win.view.focus_mode = true;
    win.address.state.open_with("");

    let mut shapes = 0;
    for _ in 0..2 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| win.draw_address_bar(ctx));
        shapes = out.shapes.len();
    }
    assert_eq!(shapes, 0, "专注模式里地址栏仍然画出了输入框");
}
