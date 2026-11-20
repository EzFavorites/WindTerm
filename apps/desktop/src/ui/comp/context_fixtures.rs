//! 视觉基线两份右键菜单的组件夹具：`menu_terminal_context` / `menu_view_context`。
//!
//! 从 `comp/mod.rs` 拆出来同样是为了 R9 的 500 行上限。夹具与生产路径共用
//! `terminal::context_rows::draw_context_rows`，所以「夹具好看、真菜单不对」
//! 这条老问题在这里不会发生。

use eframe::egui;

use super::CompId;
use crate::ui::harness;
use crate::ui::shell;
use crate::ui::terminal::context_rows::{draw_context_rows, TERMINAL_ROWS, VIEW_ROWS};
use crate::ui::TerminalWindow;

pub(in crate::ui) fn draw(win: &mut TerminalWindow, ctx: &egui::Context, comp: CompId) {
    // 三个尚未挂接的弹层：只在夹具里画，不接生产入口（用户后续提供挂载点）。
    match comp {
        CompId::ShellListPopup | CompId::OutlineMoreMenu | CompId::AddressBarDropdown => {
            draw_detached(ctx, comp);
            return;
        }
        _ => {}
    }
    // 蓝块是截图那一瞬鼠标划过的指向指示（与 hover 同色），夹具定格它以便
    // 逐字节对照。用户 2026-11-12 提供的 ⋮ 清晰截图里高亮的是 `Copy` 行；
    // `05-context-menus` 那两份没有高亮行。
    let (rows, highlighted): (&[crate::ui::terminal::context_rows::Row], Option<&'static str>) =
        match comp {
            CompId::TerminalContext => (TERMINAL_ROWS, None),
            CompId::ViewContext => (VIEW_ROWS, Some("Copy")),
            _ => return,
        };
    harness::show_canvas(ctx, |_ui| {});
    egui::Area::new(egui::Id::new("comp_context_menu"))
        .fixed_pos(egui::pos2(20.0, 26.0))
        // 夹具时钟 t=0，默认淡入会把 #333333 合成成深色；与菜单夹具一致。
        .fade_in(false)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            // 宽度不再由夹具写死：`draw_context_rows` 自己按内容
            // `set_width`（与菜单栏同一条公式），夹具与生产因此同宽。
            egui::Frame::none()
                .fill(shell::popup_bg())
                .inner_margin(egui::Margin::symmetric(4.0, 6.0))
                .rounding(4.0)
                .show(ui, |ui| {
                    let mut sink = Vec::new();
                    let lang = win.lang();
                    draw_context_rows(win, lang, ui, rows, &mut sink, highlighted);
                });
        });
}

/// 三个未挂接弹层的夹具：只出图，不进生产路径。
///
/// shell 下拉 / Outline ⋮ 菜单用菜单弹层的 Frame（#333 + 4/6 内边距），
/// 地址栏下拉是整宽行列表，宽度按视觉基线整窗地址栏量取。
fn draw_detached(ctx: &egui::Context, comp: CompId) {
    harness::show_canvas(ctx, |_ui| {});
    let title = match comp {
        CompId::ShellListPopup => "shell list",
        CompId::OutlineMoreMenu => "outline more",
        _ => "address bar",
    };
    let width = if comp == CompId::AddressBarDropdown {
        760.0
    } else if comp == CompId::OutlineMoreMenu {
        210.0
    } else {
        150.0
    };
    egui::Area::new(egui::Id::new("comp_detached_popup").with(title))
        .fixed_pos(egui::pos2(24.0, 28.0))
        .fade_in(false)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_width(width);
            egui::Frame::none()
                .fill(shell::popup_bg())
                .inner_margin(egui::Margin::symmetric(6.0, 6.0))
                .rounding(3.0)
                .show(ui, |ui| match comp {
                    // 02-scheme-list-popup-open.png：cmd 既勾选又蓝块高亮。
                    CompId::ShellListPopup => shell::draw_shell_list(ui, 0, Some("cmd")),
                    CompId::OutlineMoreMenu => {
                        shell::draw_outline_menu(term_app::i18n::Lang::En, ui)
                    }
                    CompId::AddressBarDropdown => {
                        shell::draw_address_dropdown(term_app::i18n::Lang::En, ui)
                    }
                    _ => {}
                });
        });
}
