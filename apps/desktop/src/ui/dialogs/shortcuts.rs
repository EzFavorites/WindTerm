//! 全局快捷键派发与重绑定编辑器（U-CMDPAL）。
//!
//! 唯一真相是 `term_app::shortcut_map::KeyBindingMap`：出厂表 + 用户覆盖
//! 都按**命令 ID** 键控，平台只影响展示（`Cmd` 在 macOS 显示 `Meta`，
//! Win/Linux 显示 `Ctrl`）。菜单右侧那一列也从它取，不再抄第二份字符串。
//!
//! 这里一个命令 ID 字面量都没有：`dispatch(id)` 拿映射层给出的 ID。
//! 重绑先构造候选映射、跑冲突检测，再经 `save_prefs` 写回 `shortcut_bindings`；
//! 老 prefs 的 `shortcut_overrides`（数组下标）在 `build_runtime` 里一次性迁移。
//!
//! **两段式键**（`Alt+W, Alt+H`）同样来自映射层：`sequence_table` 从绑定派生
//! 运行期表，纯层 `KeySeqState` 判定「这一击是前缀 / 命中 / 放过」。单段与
//! 多段共用一张绑定表，只是判定路径不同。

use crate::ui::kit::window::{DialogId, DialogWindow};
use crate::ui::*;
use super::settings_live::save_prefs;
use term_app::shortcuts::{self, Chord, SeqStep};
use term_app::shortcut_map::{KeyBindingMap, Platform};

/// 命令 ID 的展示名。设置页那张表仍带人类可读 label，新映射层只认 ID。
fn label_for(id: &str) -> String {
    term_app::menu::DEFAULT_SHORTCUTS
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.label.to_string())
        .unwrap_or_else(|| id.to_string())
}

fn static_id(id: &str) -> Option<&'static str> {
    term_app::shortcut_map::DEFAULT_BINDINGS
        .iter()
        .find(|r| r.id == id)
        .map(|r| r.id)
}

/// 从 prefs 构造映射层与运行期键序表。老格式（数组下标）在这里一次性迁移。
pub(in crate::ui) fn build_runtime(
    prefs: &term_profile::prefs::Prefs,
) -> (KeyBindingMap, Vec<(Vec<Chord>, &'static str)>) {
    let platform = match std::env::var("WINDTERM_SHORTCUT_PLATFORM").as_deref() {
        Ok("windows") => Platform::Windows,
        Ok("linux") => Platform::Linux,
        Ok("macos") => Platform::Macos,
        _ => Platform::current(),
    };
    let user = if prefs.shortcut_bindings.is_empty() {
        term_app::shortcut_map::migrate_legacy(&prefs.shortcut_overrides)
    } else {
        prefs.shortcut_bindings.clone()
    };
    let map = KeyBindingMap::new(&user, platform);
    let seq = sequence_table(&map);
    (map, seq)
}

/// 键序表从映射层派生，不再单独抄一份 `DEFAULT_SHORTCUT_SEQS`。
fn sequence_table(map: &KeyBindingMap) -> Vec<(Vec<Chord>, &'static str)> {
    let mut table = Vec::new();
    for (id, chord) in map.rows() {
        if let (Some(id), Some(seq)) = (static_id(&id), shortcuts::parse_chord_seq(&chord)) {
            if seq.len() > 1 && term_app::capability::is_implemented(id) {
                table.push((seq, id));
            }
        }
    }
    table
}

/// 把 egui 的键名折成纯层认识的大写名。不在纯层里认 egui（4f）。
fn key_name(key: egui::Key) -> Option<&'static str> {
    use egui::Key as K;
    Some(match key {
        K::P => "P", K::O => "O", K::K => "K", K::F => "F", K::N => "N",
        K::T => "T", K::W => "W", K::L => "L", K::C => "C", K::V => "V",
        K::A => "A", K::G => "G", K::H => "H", K::J => "J",
        // margin 五列右键键位 `Alt+T, Alt+B/N/F/S/T` 的后继：缺了就在
        // `if let Some(name)` 处静默丢掉，菜单写着键却永远按不响。
        K::B => "B", K::S => "S",
        // `Alt+[` / `Alt+]` 视图切换：缺这两行时文档写的键在派发处被静默丢掉。
        K::OpenBracket => "OpenBracket", K::CloseBracket => "CloseBracket",
        // 2026-11-11：补 `M`。`Alt+M, Alt+H`（Hex 视图）与 `Alt+M, Alt+W`
        // （Word Wrap）的第一段就是 `M`，缺这一行时那一击会在调用方
        // `if let Some(name)` 处**静默丢掉**：不开火、不进 pressed、
        // 也不消费事件——用户感受是「按了没反应」，而 macOS 上
        // `Event::Text` 还会把组合字符（`µ`）原样送进 PTY。
        K::M => "M",
        K::F1 => "F1", K::F2 => "F2", K::F3 => "F3", K::F4 => "F4",
        K::F5 => "F5", K::F6 => "F6", K::F7 => "F7", K::F8 => "F8",
        K::F9 => "F9", K::F10 => "F10", K::F11 => "F11", K::F12 => "F12",
        K::Enter => "Enter", K::Escape => "Escape", K::Tab => "Tab",
        K::Space => "Space", K::Backspace => "Backspace", K::Delete => "Delete",
        K::Home => "Home", K::End => "End",
        K::ArrowUp => "ArrowUp", K::ArrowDown => "ArrowDown",
        K::ArrowLeft => "ArrowLeft", K::ArrowRight => "ArrowRight",
        K::PageUp => "PageUp", K::PageDown => "PageDown",
        _ => return None,
    })
}

/// 运行期装的两段式键：`(整段按键, 命令 ID)`。
///
/// 两件事在这里定死：
/// 1. **解析失败就 panic**，不许静默跳过——表写错了是数据缺陷，藏着它只会让
///    用户「按了没反应」。
/// 2. **只装已接线的命令**。`DEFAULT_SHORTCUT_SEQS` 有七条，其中三条今天
///    `Action::parse` 返回 None（`dispatch` 里直接 return）。把它们的键序也装进来，
///    第一段（如 `Cmd+T`）就会进前缀态白吞按键，还永远等不到后继动作。
pub(in crate::ui) fn runtime_sequence_table() -> Vec<(Vec<Chord>, &'static str)> {
    sequence_table(&KeyBindingMap::new(&[], Platform::current()))
}

/// 本帧收集到的一次按下：纯层和弦 + 还原事件所需的 egui 原件。
struct Press {
    chord: Chord,
    key: egui::Key,
    modifiers: egui::Modifiers,
    repeat: bool,
}

impl TerminalWindow {
    /// 每帧开头调用：捕获全局组合键。浮层打开时它们自己解释按键，
    /// 但 `Cmd+Shift+P` 属于全局（从任何浮层里都能再叫出命令面板）。
    ///
    /// 两条通路，顺序固定：
    /// 1. **键序状态机**（两段式键）：`Pending` / `Fire` 都吃掉这一击并停手；
    /// 2. **单段匹配**（覆盖表 → 出厂表）：命中则吃掉。
    ///
    /// `Pass` 才轮到第 2 步，两步都不命中就什么都不吃——`pane.rs` 照常把字符
    /// 写进 PTY。
    pub(in crate::ui) fn handle_global_shortcuts(&mut self, ctx: &egui::Context) {
        // egui 的帧时间（秒）。**不用墙钟**：夹具把 `i.time` 钉成 0.0，
        // 用 `Instant::now()` 会让截图/回放随真实时间抖动。
        let now_ms = (ctx.input(|i| i.time) * 1000.0) as u64;
        let mut pressed: Vec<Press> = Vec::new();
        ctx.input(|i| {
            for event in &i.events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    repeat,
                    ..
                } = event
                {
                    if let Some(name) = key_name(*key) {
                        pressed.push(Press {
                            chord: shortcuts::chord(
                                modifiers.command,
                                modifiers.ctrl,
                                modifiers.shift,
                                modifiers.alt,
                                name,
                            ),
                            key: *key,
                            modifiers: *modifiers,
                            repeat: *repeat,
                        });
                    }
                }
            }
        });

        // 捕获态优先：正在重新绑定时，这一次的按键不触发任何命令。
        // 先清掉键序前缀：用户「点 Rebind → 按了第一段就放弃 → 关窗」会留下
        // 跨帧 pending，下一次按 H 就会意外分屏。
        if let Some(id) = self.shortcut_capture.clone() {
            self.keyseq.clear();
            if let Some(press) = pressed.first() {
                if press.chord.cmd || press.chord.alt {
                    let mut user = self.shortcuts.user_bindings();
                    user.retain(|(key, _)| key != &id);
                    user.push((id.clone(), press.chord.text()));
                    let trial = KeyBindingMap::new(&user, self.shortcuts.platform());
                    let clash = trial.conflicts().into_iter().find(|(_, a, b)| {
                        a == &id || b == &id
                    });
                    if let Some((chord, _, other)) = clash {
                        self.log_note = Some(format!(
                            "快捷键 {chord} 已被 {} 占用",
                            label_for(&other)
                        ));
                    } else {
                        self.shortcuts = trial;
                        // 重绑立即落盘，重启后仍生效。
                        self.persist_shortcuts();
                    }
                }
                self.shortcut_capture = None;
            }
            return;
        }

        // 本帧吃掉的按下（下标指回 `pressed`）。非空 = 还要抹掉对应事件。
        let mut consumed: Vec<usize> = Vec::new();

        // ① 键序状态机。`repeat` 不进：按住 Alt+W 不该反复进前缀、更不该反复开火。
        for (idx, press) in pressed.iter().enumerate() {
            if press.repeat {
                continue;
            }
            match self.keyseq.feed(&self.seq_table, press.chord, now_ms) {
                SeqStep::Pending => {
                    consumed.push(idx);
                    break;
                }
                SeqStep::Fire(i) => {
                    // 先把 ID 拷出来：`dispatch` 要可变借 `self`，而表还借着它。
                    let id = self.seq_table[i].1;
                    consumed.push(idx);
                    self.dispatch(id);
                    break;
                }
                SeqStep::Pass => {}
            }
        }

        // ② 单段匹配：映射层同时管出厂表与用户覆盖，调用方不再知道它们的存在。
        if consumed.is_empty() {
            for (idx, press) in pressed.iter().enumerate() {
                let Some(id) = self.shortcuts.dispatch(&[press.chord]).map(str::to_string) else {
                    continue;
                };
                consumed.push(idx);
                self.dispatch(&id);
                break;
            }
        }

        if consumed.is_empty() {
            return;
        }

        let eaten: Vec<(egui::Modifiers, egui::Key)> = consumed
            .iter()
            .map(|i| (pressed[*i].modifiers, pressed[*i].key))
            .collect();
        eat_shortcut_events(ctx, &eaten);
    }
}

/// 把「这一帧已经开火的按下」连同它带出来的文本一起从输入事件里抹掉。
///
/// # 为什么只抹 `Key` 不够（2026-11-11 逐层读源码确认，不是猜）
///
/// 一条带 `Alt` 的按下在 macOS 上**会**带一个 `Event::Text`，两层都不拦：
///
/// 1. `winit 0.30.13`（`Cargo.lock` 里就是这版）`platform_impl/macos/event.rs:154`：
///    `let text = if is_press { logical_key.to_text() } else { None };`
///    —— `KeyW` 在 `code_to_key` 里落到 `_ => Key::Unidentified`（`:228`），走
///    `create_key_event` 的「逻辑键」分支（`:121`–`:149`），`has_ctrl/has_cmd`
///    都是 false 时取 `Key::Character(带 Alt 效果的字符)`（`:135`–`:138`，
///    注释明说 **故意不排除 alt**）。于是 `text = Some(组合字符)`——US 布局下
///    `Option+M` 就是 `µ`（具体字符随键盘布局变，但**非空且可打印**这一点不变，
///    而 `is_printable_char` 正是 egui-winit 唯一的准入门槛）。
/// 2. `egui-winit 0.29.1`（`src/lib.rs:794`–`:801`）只做一件事：
///    `let is_cmd = ctrl || command || mac_cmd; if pressed && !is_cmd { push(Text) }`
///    —— **`alt` 不在 `is_cmd` 里**，所以这个 Text 原样进 `InputState::events`。
///
/// 而 `pane.rs:237` 把 `Event::Text` 无条件写进 PTY。也就是说：不抹的话，
/// 按 `Alt+W` 开分屏的同时，PTY 里会多出一个 `∑`。
///
/// # 为什么要整帧抹
///
/// `Event::Text` 不带键身份（只有一串字符），无法判断它是不是这次按下的副产物。
/// 所以只在「本帧确实消费过按键」时抹，代价是同一帧里别的文本事件也会被牺牲
/// —— egui 一帧内的按键序列本来就不保证，这个代价比往 PTY 里灌乱码小得多。
fn eat_shortcut_events(ctx: &egui::Context, eaten: &[(egui::Modifiers, egui::Key)]) {
    if eaten.is_empty() {
        return;
    }
    ctx.input_mut(|i| {
        i.events.retain(|event| match event {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => !eaten.iter().any(|(m, k)| m == modifiers && k == key),
            egui::Event::Text(_) => false,
            _ => true,
        });
    });
}

impl TerminalWindow {
    /// 把用户覆盖按命令 ID 写回 `prefs.json`，使重启后仍生效。
    fn persist_shortcuts(&mut self) {
        let bindings = self.shortcuts.user_bindings();
        save_prefs(|p| p.shortcut_bindings = bindings);
    }

    /// 绑定表窗口（默认关）。列出全部绑定、允许重绑、能清空覆盖。
    pub(in crate::ui) fn draw_shortcuts(&mut self, ctx: &egui::Context) {
        if !self.view.show_shortcuts {
            return;
        }
        let open = DialogWindow::new(DialogId::Shortcuts, "Keyboard Shortcuts")
        .show(ctx, |ui| {
            ui.label("Click Rebind, then press the new combination (Cmd/Ctrl/Alt + key).");
            ui.separator();
            let mut rebind: Option<String> = None;
            let rows = self.shortcuts.rows();
            egui::Grid::new("shortcut_grid").num_columns(3).show(ui, |ui| {
                ui.label("Action");
                ui.label("Shortcut");
                ui.label("");
                ui.end_row();
                for (id, text) in &rows {
                    ui.label(label_for(id));
                    let capturing = self.shortcut_capture.as_deref() == Some(id.as_str());
                    ui.colored_label(
                        if capturing { egui::Color32::YELLOW } else { ui.visuals().text_color() },
                        if capturing { "press a key…".to_string() } else { text.clone() },
                    );
                    if ui.button("Rebind").clicked() {
                        rebind = Some(id.clone());
                    }
                    ui.end_row();
                }
            });
            if let Some(id) = rebind {
                self.shortcut_capture = Some(id);
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Reset to defaults").clicked() {
                    self.shortcuts = KeyBindingMap::new(&[], self.shortcuts.platform());
                    self.shortcut_capture = None;
                    // 清空覆盖同样写回，之后所有条目回到出厂和弦。
                    self.persist_shortcuts();
                }
                if ui.button("Close").clicked() {
                    self.view.show_shortcuts = false;
                }
            });
        });
        if !open {
            self.view.show_shortcuts = false;
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn every_default_binding_parses_to_a_chord() {
        for s in term_app::menu::DEFAULT_SHORTCUTS {
            assert!(term_app::shortcuts::parse_chord(s.chord).is_some(), "{}", s.chord);
        }
    }

    #[test]
    fn every_binding_targets_a_wired_command() {
        // 绑定表里不能挂着点了没反应的置灰命令。
        for s in term_app::menu::DEFAULT_SHORTCUTS {
            assert!(term_app::capability::is_implemented(s.id), "{}", s.id);
        }
    }

    #[test]
    fn command_palette_shortcut_is_cmd_shift_p() {
        let p = term_app::shortcuts::chord(true, false, true, false, "P");
        // ui/ 不得出现命令 ID 字面量，连测试里也不行。
        // 用数据层的查询函数反查，比写字面量更经得起菜单改名。
        let id = term_app::menu::shortcut_for("Cmd+Shift+P").unwrap();
        assert_eq!(term_app::capability::describe(id).feature, Some(term_app::action::FeatureId::Palette));
        assert!(term_app::menu::DEFAULT_SHORTCUTS
            .iter()
            .any(|s| s.id == id && term_app::shortcuts::matches(s.chord, p)));
    }

    /// 运行期表里的每一段，`key_name` 都必须折得出来。
    ///
    /// 这条 2026-11-11 首次跑会立刻红：`Alt+M` 的 `M` 不在 `key_name` 的映射里，
    /// 而那一击在 `handle_global_shortcuts` 的 `if let Some(name)` 处会被
    /// **静默丢掉**——不开火、不消费、Text 照样进 PTY。补 `K::M` 才绿。
    /// 把静默缺口变成红灯，是这条测试存在的唯一理由。
    #[test]
    fn the_projection_can_translate_every_sequence_segment() {
        let mut bad = Vec::new();
        for (seq, id) in super::runtime_sequence_table() {
            for chord in &seq {
                let translated = egui::Key::ALL
                    .iter()
                    .filter_map(|k| super::key_name(*k))
                    .any(|name| name == chord.key);
                if !translated {
                    bad.push(format!(
                        "{id}：段 {:?} 的键名折不出对应的 egui::Key",
                        chord.text()
                    ));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "键序里有一段根本收不到按键——按下去只会静默丢掉：\n{}",
            bad.join("\n")
        );
    }

    /// 运行期表只装「已接线」的键序，且条数对得上。
    ///
    /// 未接线的那三条若也装进来，它们的第一段（如 `Cmd+T`）会进前缀态白吞按键，
    /// 而且永远等不到后继动作。这里用 `DEFAULT_SHORTCUT_SEQS` + `is_implemented`
    /// 现算，不写命令 ID 字面量。
    #[test]
    fn the_runtime_table_leaves_out_the_unwired_sequences() {
        let table = super::runtime_sequence_table();
        let data = term_app::menu::DEFAULT_SHORTCUT_SEQS;
        let wired = data
            .iter()
            .filter(|s| term_app::capability::is_implemented(s.id))
            .count();
        assert_eq!(
            table.len(),
            wired,
            "运行期表应恰好等于「已接线」的键序条数（数据 {} 条，其中已接线 {wired} 条）",
            data.len()
        );
        assert!(wired > 0, "一条都没接线时这条断言没有意义，说明数据表塌了");
        for s in data {
            if !term_app::capability::is_implemented(s.id) {
                assert!(
                    !table.iter().any(|(_, id)| *id == s.id),
                    "{} 还没接线，不该进运行期表（它的第一段会白吞按键）",
                    s.id
                );
            }
        }
    }

    /// **本文件唯一一条跨进 egui 输入队列的断言**：开火的快捷键不许把
    /// 它的文本事件漏进 PTY。
    ///
    /// `handle_global_shortcuts` 的职责不止「判命中」——命中之后还要把
    /// `Event::Key` 与 `Event::Text` 从 `InputState::events` 里删掉，否则
    /// `pane.rs:237` 会把 `Alt` 组合字符原样写进 PTY（见 [`super::eat_shortcut_events`]
    /// 里逐层引到的 winit / egui-winit 行号）。这里用真的 `Context` 造一帧
    /// **顺序与 egui-winit 产出完全一致**的事件（先 `Key` 后 `Text`），再调生产函数——
    /// 断言对象是「剩下的输入队列长什么样」，不是我自己拼的中间值。
    ///
    /// 反证方向：把 `super::eat_shortcut_events` 里的 `Event::Text(_) => false` 改回
    /// `true`，这条立刻红在 `has_text = true`。
    #[test]
    fn a_fired_shortcut_does_not_leak_its_text_into_the_pty() {
        let alt_w = egui::Modifiers {
            alt: true,
            ..Default::default()
        };
        let plain = egui::Modifiers::default();
        let press = |key: egui::Key, modifiers: egui::Modifiers| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        };
        let input = egui::RawInput {
            modifiers: alt_w,
            events: vec![
                press(egui::Key::W, alt_w),
                // winit 会把 `Option+W` 的组合字符塞在这里（US 布局是 `∑`，
                // 但字符本身无关紧要：egui-winit 只校验「非空且可打印」）。
                egui::Event::Text("∑".to_owned()),
                // 同一帧里的另一击：不是快捷键，必须原样留下。
                press(egui::Key::A, plain),
            ],
            ..Default::default()
        };

        let ctx = egui::Context::default();
        let mut observed = (false, false, false);
        let _ = ctx.run(input, |ctx| {
            super::eat_shortcut_events(ctx, &[(alt_w, egui::Key::W)]);
            observed = ctx.input(|i| {
                let has_text = i.events.iter().any(|e| matches!(e, egui::Event::Text(_)));
                let has_key = |k: egui::Key| {
                    i.events.iter().any(
                        |e| matches!(e, egui::Event::Key { key, pressed: true, .. } if *key == k),
                    )
                };
                (has_text, has_key(egui::Key::W), has_key(egui::Key::A))
            });
        });

        assert!(
            !observed.0,
            "开火之后这一帧还留着 `Event::Text`：`pane.rs` 会把组合字符写进 PTY，\
             用户按 `Alt+W` 分屏时会多打出 `∑`"
        );
        assert!(
            !observed.1,
            "开火的那一击 `Alt+W` 还留在队列里，PTY 会再收一次"
        );
        assert!(
            observed.2,
            "顺手把无关的 `A` 也抹了——抹除范围超出了「本帧消费过的按键 + 文本事件」"
        );
    }
}
