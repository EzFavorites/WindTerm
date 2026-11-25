//! 悬停浮层的**帧间锚点锁存**。
//!
//! `egui::Area` 每帧把 `anchor` 设成当前 `hover_pos + 偏移` 会让浮层追着指针跑：
//! 指针往浮层移动 → 锚点跟着移动 → 命中区随之失配 → 浮层在点到之前就消失。
//! 这里把锚点与「当时展示的内容」锁进帧间状态：token 不变就复用旧锚点；指针进入
//! 上一次的浮层矩形时直接复用旧内容继续 show。只有 token（哪一行、哪一列、哪个
//! token）真的变了才重设锚点。

use eframe::egui;

/// 悬停浮层的 egui Id：**只按窗格（调用处的 `ui.unique_id()`）+ 种类定型**。
///
/// 用 `unique_id()` 而不是 `id()`：同一父 Ui 下多个 `new_child` 的 `id()` 都叫
/// `"child"`，分屏的两个窗格会共用一把钥匙；`unique_id()` 带自增序号，才是
/// 一块窗格一份。
///
/// 指针移进浮层后，终端侧的 row/col 会变、甚至根本算不出 token；若 Id 里带
/// row/col，`Latch` 就取不到上一帧锁住的锚点与内容，浮层当场闪退或点错 token。
/// `row:col:token` 只作为 [`resolve`] 的参数，绝不参与 Id。
pub(in crate::ui) fn hover_id(ui_id: egui::Id, kind: &str) -> egui::Id {
    egui::Id::new(("hover_token", ui_id, kind))
}

#[derive(Clone)]
pub(in crate::ui) struct Latch<T> {
    token: String,
    anchor: egui::Pos2,
    rect: Option<egui::Rect>,
    payload: T,
}

/// 解析本帧的锚点与要展示的内容。
///
/// 返回 `(anchor, payload)`：`payload` 可能是本帧新算的，也可能是指针进入浮层时
/// 从上一帧留住的——调用方不要在本帧指针处再判定一次命中，否则刚锁住又丢掉。
pub(in crate::ui) fn resolve<T: Clone + Send + Sync + 'static>(
    ui: &egui::Ui,
    id: egui::Id,
    token: &str,
    pos: egui::Pos2,
    offset: egui::Vec2,
    fresh: T,
) -> (egui::Pos2, T) {
    let old = ui.data(|d| d.get_temp::<Latch<T>>(id));
    let inside = old
        .as_ref()
        .and_then(|state| state.rect)
        .is_some_and(|rect| rect.expand(2.0).contains(pos));
    if let Some(state) = old {
        // 指针还在同一 token 上：锚点不许动。指针进了浮层：token 可能因为
        // 「指针已离开终端格」而算不出来，但这一次必须继续展示旧内容。
        if state.token == token || inside {
            let anchor = state.anchor;
            let payload = state.payload.clone();
            ui.data_mut(|d| {
                d.insert_temp(id, Latch {
                    token: state.token.clone(),
                    anchor,
                    rect: state.rect,
                    payload: payload.clone(),
                });
            });
            return (anchor, payload);
        }
    }
    let anchor = pos + offset;
    ui.data_mut(|d| {
        d.insert_temp(id, Latch {
            token: token.to_string(),
            anchor,
            rect: None,
            payload: fresh.clone(),
        });
    });
    (anchor, fresh)
}

/// `Area::show` 之后记下浮层矩形，下一帧用它判「指针是不是进来了」。
pub(in crate::ui) fn remember<T: Clone + Send + Sync + 'static>(ui: &egui::Ui, id: egui::Id, rect: egui::Rect) {
    ui.data_mut(|d| {
        if let Some(mut state) = d.get_temp::<Latch<T>>(id) {
            state.rect = Some(rect);
            d.insert_temp(id, state);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Eq, Debug)]
    struct Payload(&'static str);

    #[test]
    fn the_anchor_is_latched_until_the_token_changes() {
        let id = hover_id(egui::Id::new("pane_slot_1"), "ip");
        egui::__run_test_ui(|ui| {
            let (a1, p1) = resolve(&ui, id, "3:7:10.0.0.9", egui::pos2(100.0, 60.0), egui::vec2(10.0, 10.0), Payload("ip"));
            assert_eq!(a1, egui::pos2(110.0, 70.0));
            assert_eq!(p1, Payload("ip"));
            remember::<Payload>(ui, id, egui::Rect::from_min_size(a1, egui::vec2(120.0, 80.0)));
            // 同一 token、指针在终端里挪了几格：锚点与内容都不动。
            let (a2, _) = resolve(ui, id, "3:7:10.0.0.9", egui::pos2(140.0, 80.0), egui::vec2(10.0, 10.0), Payload("other"));
            assert_eq!(a2, a1, "token 没变就不许重设锚点");
            // token 变了才重设。
            // 这次指针在浮层外，token 也变了：才允许重设锚点。
            let (a3, p3) = resolve(ui, id, "3:9:1.2.3.4", egui::pos2(260.0, 200.0), egui::vec2(10.0, 10.0), Payload("next"));
            assert_eq!(a3, egui::pos2(270.0, 210.0));
            assert_eq!(p3, Payload("next"));
        });
    }

    #[test]
    fn a_pointer_inside_the_popup_keeps_the_old_anchor_and_payload() {
        let id = hover_id(egui::Id::new("pane_slot_2"), "builtin");
        egui::__run_test_ui(|ui| {
            let (anchor, _) = resolve(ui, id, "1:2:0xFF", egui::pos2(40.0, 20.0), egui::vec2(10.0, 10.0), Payload("hex"));
            remember::<Payload>(ui, id, egui::Rect::from_min_size(anchor, egui::vec2(140.0, 90.0)));
            // 指针进浮层后，终端侧的命中自然没了（fresh=None），但仍要继续 show。
            let (kept, payload) = resolve(ui, id, "", egui::pos2(100.0, 60.0), egui::vec2(10.0, 10.0), Payload("none"));
            assert_eq!(kept, anchor);
            assert_eq!(payload, Payload("hex"), "浮层里的按钮必须还在，不能一移动就消失");
        });
    }

    /// 回归：指针从终端格移进浮层那一瞬，row/col 变了，但 id 必须不变。
    /// 旧写法把 row/col 编进 `egui::Id`，`Latch` 取不到→闪退 / 点错 token。
    #[test]
    fn moving_the_pointer_into_the_popup_does_not_change_the_id_or_the_anchor() {
        let pane = egui::Id::new("pane_slot_3");
        let id = hover_id(pane, "ip_menu");
        // 调用方每帧都会重算 row:col:token，但只传给 resolve，不传给 Id。
        let id_after_pointer_moves = hover_id(pane, "ip_menu");
        assert_eq!(id_after_pointer_moves, id, "指针位移不许打穿锁存");
        assert_ne!(id, hover_id(pane, "builtin_tooltip"), "同窗格不同浮层各一份锁存");
        assert_ne!(id, hover_id(egui::Id::new("pane_slot_4"), "ip_menu"), "两窗格互不串");

        egui::__run_test_ui(|ui| {
            // 指针在 3:7 命中 IP：建浮层并记下它的矩形。
            let token_at_cell = "3:7:10.0.0.9";
            let (anchor, payload) = resolve(ui, id, token_at_cell, egui::pos2(100.0, 60.0), egui::vec2(10.0, 10.0), Payload("ip"));
            assert_eq!(payload, Payload("ip"));
            remember::<Payload>(ui, id, egui::Rect::from_min_size(anchor, egui::vec2(120.0, 80.0)));

            // 模拟指针进入浮层：终端侧算出的 row/col 不再是 3:7（这里用 9:1 表示），
            // token 也退化成空；但 id 仍是同一把钥匙，锚点与内容必须原样留住。
            let token_after_enter = "";
            let (kept_anchor, kept_payload) = resolve(
                ui, id_after_pointer_moves, token_after_enter,
                egui::pos2(150.0, 90.0), egui::vec2(10.0, 10.0), Payload("none"),
            );
            assert_eq!(kept_anchor, anchor, "锚点不能被新指针位置带跑");
            assert_eq!(kept_payload, Payload("ip"), "浮层里的按钮不能闪退");
        });
    }
}
