//! 全局快捷键的**前置门控**与事件清理（UI 外壳审查批次）。
//!
//! `handle_global_shortcuts` 过去每帧无条件跑在浮层之前：命令面板 / Goto /
//! 工作台 / 锁屏 / 传输弹层里的 Enter、↑、↓ 会被它当成全局单段键吃掉，
//! 连带 `Event::Text` 一起删掉，浮层里的输入框与列表于是「按了没反应」。
//!
//! 两条规矩收在这里：
//!
//! 1. **门控**：浮层 / 对话框开着时，只有带 `Cmd`/`Ctrl`/`Alt` 的**真全局**
//!    组合键还允许派发；单段 `Return` / `↑` / `↓` 既不派发也不吞，原样留给
//!    浮层自己的输入框与列表。
//! 2. **精确清理**：只删本次**确实消费过**的那几枚 `Key`；`Text` 仅在被消费
//!    的键真的会产生文本（Alt/Shift 组合）时删——纯 `Cmd`/`Ctrl` 组合本来
//!    就不会带 `Text`，没必要连同一帧里浮层的输入一起抹掉。

use eframe::egui;

/// 这一击在「浮层开着」的前提下还能不能派发成全局快捷键。
///
/// `overlay = false`（没有任何浮层）时恒放行，保持既有行为一字不变；
/// `overlay = true` 时只放行带修饰键的真全局组合，并额外挡掉三枚导航键——
/// 即使哪天有人给它们加了绑定，也不许越过浮层。
pub(in crate::ui) fn dispatchable(
    key: egui::Key,
    modifiers: egui::Modifiers,
    overlay: bool,
) -> bool {
    if !overlay {
        return true;
    }
    let global_chord = modifiers.command || modifiers.ctrl || modifiers.alt;
    let single_navigation = matches!(
        key,
        egui::Key::Enter | egui::Key::ArrowUp | egui::Key::ArrowDown
    );
    global_chord && !single_navigation
}

/// 只删本次实际消费的键，以及**紧跟在它后面**的那一条 `Text`。
///
/// `Event::Text` 不携带按键身份，整帧清掉会把浮层输入框里刚敲的字一起抹掉
/// （这正是命令面板 / Goto 的 Enter 与方向键失效的另一半原因）。egui-winit
/// 产出的顺序是「Key → Text」紧邻两条，所以按相邻关系定位即可：被消费的
/// Key 后面那一条 Text 是它的副产物，其余 Text 原样留下。
pub(in crate::ui) fn eat(
    ctx: &egui::Context,
    eaten: &[(egui::Modifiers, egui::Key)],
) {
    if eaten.is_empty() {
        return;
    }
    ctx.input_mut(|i| {
        let events = std::mem::take(&mut i.events);
        let mut kept = Vec::with_capacity(events.len());
        let mut consumed_text = false;
        for event in events {
            match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if eaten.iter().any(|(m, k)| *m == modifiers && *k == key) => {
                    // 这一击被消费：它后面紧邻的那条 Text 是它带出来的组合字符。
                    consumed_text = true;
                }
                egui::Event::Text(_) if consumed_text => {
                    consumed_text = false;
                }
                other => {
                    consumed_text = false;
                    kept.push(other);
                }
            }
        }
        i.events = kept;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(alt: bool, command: bool) -> egui::Modifiers {
        egui::Modifiers {
            alt,
            command,
            mac_cmd: command,
            ..Default::default()
        }
    }

    /// 四象限：浮层开/关 × 单段导航键/真全局组合键。
    ///
    /// 这四格正是审查发现的那一刀：过去浮层开着时单段 Enter/↑/↓ 也会被
    /// 派发并吞掉，于是命令面板 / Goto / Center 的键盘失效。
    #[test]
    fn four_quadrants_of_shortcut_gating() {
        let plain = mods(false, false);
        let alt = mods(true, false);
        let cmd = mods(false, true);

        // ① 没有浮层：单段导航键照旧放行（没有绑定就不会被消费）。
        assert!(dispatchable(egui::Key::Enter, plain, false));
        assert!(dispatchable(egui::Key::ArrowUp, plain, false));
        assert!(dispatchable(egui::Key::ArrowDown, plain, false));
        // ② 没有浮层：组合键也放行。
        assert!(dispatchable(egui::Key::W, alt, false));
        assert!(dispatchable(egui::Key::P, cmd, false));

        // ③ 有浮层：单段 Enter/↑/↓ 一律不放行——既不派发也不吞。
        assert!(!dispatchable(egui::Key::Enter, plain, true));
        assert!(!dispatchable(egui::Key::ArrowUp, plain, true));
        assert!(!dispatchable(egui::Key::ArrowDown, plain, true));
        // ④ 有浮层：真全局组合键仍然放行（从浮层里也能叫出命令面板）。
        assert!(dispatchable(egui::Key::W, alt, true));
        assert!(dispatchable(egui::Key::P, cmd, true));
        // 浮层开着时其它单段字符也不算全局。
        assert!(!dispatchable(egui::Key::A, plain, true));
    }

    /// 事件往返：被消费的 `Key` 与它带出来的 `Text` 删掉；同帧里浮层自己的
    /// Enter / ↑ 与无关文本原样留下。
    #[test]
    fn eating_only_removes_the_consumed_keys_and_their_text() {
        let alt = mods(true, false);
        let plain = mods(false, false);
        let press = |key, m| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: m,
        };
        let input = egui::RawInput {
            events: vec![
                press(egui::Key::W, alt),
                egui::Event::Text("∑".to_owned()),
                // 浮层自己的导航键 + 文本：必须原样留下。
                press(egui::Key::Enter, plain),
                press(egui::Key::ArrowUp, plain),
                egui::Event::Text("abc".to_owned()),
            ],
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut observed = (false, false, false, false);
        let _ = ctx.run(input, |ctx| {
            eat(ctx, &[(alt, egui::Key::W)]);
            observed = ctx.input(|i| {
                let has = |k| {
                    i.events.iter().any(
                        |e| matches!(e, egui::Event::Key { key, pressed: true, .. } if *key == k),
                    )
                };
                let texts: Vec<_> = i
                    .events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Text(s) => Some(s.as_str()),
                        _ => None,
                    })
                    .collect();
                (
                    has(egui::Key::W),
                    has(egui::Key::Enter),
                    has(egui::Key::ArrowUp),
                    texts == ["abc"],
                )
            });
        });
        assert!(!observed.0, "被消费的 Alt+W 还留在队列里");
        assert!(observed.1, "浮层的 Enter 被顺手吞掉了");
        assert!(observed.2, "浮层的 ↑ 被顺手吞掉了");
        assert!(observed.3, "无关文本被抹掉，或 Alt+W 的 ∑ 没删干净");
    }

    /// Cmd/Ctrl 组合本来就不带 Text：空的清理表与纯修饰键消费都不许动文本。
    #[test]
    fn a_command_chord_does_not_eat_unrelated_text() {
        let cmd = mods(false, true);
        let input = egui::RawInput {
            events: vec![
                egui::Event::Text("keep".to_owned()),
                egui::Event::Key {
                    key: egui::Key::P,
                    physical_key: Some(egui::Key::P),
                    pressed: true,
                    repeat: false,
                    modifiers: cmd,
                },
            ],
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut kept = false;
        let _ = ctx.run(input, |ctx| {
            eat(ctx, &[(cmd, egui::Key::P)]);
            kept = ctx.input(|i| {
                i.events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Text(s) if s == "keep"))
            });
        });
        assert!(kept, "Cmd 组合没有对应的 Text，不该把浮层的文字一起删掉");
    }
}
