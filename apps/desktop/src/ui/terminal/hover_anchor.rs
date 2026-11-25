//! 悬停浮层的**帧间锚点锁存**。
//!
//! `egui::Area` 每帧把 `anchor` 设成当前 `hover_pos + 偏移` 会让浮层追着指针跑：
//! 指针往浮层移动 → 锚点跟着移动 → 命中区随之失配 → 浮层在点到之前就消失。
//! 这里把锚点与「当时展示的内容」锁进帧间状态：token 不变就复用旧锚点；指针进入
//! 上一次的浮层矩形时直接复用旧内容继续 show。只有 token（哪一行、哪一列、哪个
//! token）真的变了才重设锚点。

use eframe::egui;

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
        let id = egui::Id::new(("hover_token", 3, 7, "ip"));
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
        let id = egui::Id::new(("hover_token", 1, 2, "builtin"));
        egui::__run_test_ui(|ui| {
            let (anchor, _) = resolve(ui, id, "1:2:0xFF", egui::pos2(40.0, 20.0), egui::vec2(10.0, 10.0), Payload("hex"));
            remember::<Payload>(ui, id, egui::Rect::from_min_size(anchor, egui::vec2(140.0, 90.0)));
            // 指针进浮层后，终端侧的命中自然没了（fresh=None），但仍要继续 show。
            let (kept, payload) = resolve(ui, id, "", egui::pos2(100.0, 60.0), egui::vec2(10.0, 10.0), Payload("none"));
            assert_eq!(kept, anchor);
            assert_eq!(payload, Payload("hex"), "浮层里的按钮必须还在，不能一移动就消失");
        });
    }
}
