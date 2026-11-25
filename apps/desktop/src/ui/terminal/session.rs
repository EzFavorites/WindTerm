//! `Tab` 的会话方法：**UI 里唯一碰控制面的地方**（ADR-0019）。
//!
//! 为什么单独一个文件而不是散在各个绘制函数里：改之前 `tab.handle` 在 12 个
//! 文件里被用了 25 次，每次都是「拿到句柄 → 调它的某个方法」。句柄一换，
//! 25 处全要改，且每处都自带一份「没有句柄怎么办」的分支。现在把「怎么办」
//! 收在这一个文件里：有会话就转发，没有就返回 `false` / `None` / 默认值。
//!
//! 一条纪律：**这个文件之外不认识 `SessionPool`**。想在 UI 里加一个新动作，
//! 在这里加一个方法，而不是在绘制代码里 `tab.pool().xxx()`——否则下一次
//! 换控制面又要改 25 处。

use std::path::{Path, PathBuf};

use term_core::coords::ViewRange;
use term_core::grid::ScreenImage;
use term_core::session_runtime::{LogStatus, SessionId, SessionStatus};

use crate::ui::*;
use term_app::trigger::{Rule, TriggerAction, TriggerOn};

/// 宿主注入的「现在」（Unix 秒）。触发器内核不读墙钟，由这里统一提供，
/// 冷却与空闲判定都用它。`SystemTime` 可能被 NTP 回拨，内核的 `allow()` 已对
/// `now < last` 放行，避免规则被永久卡死。
fn now_secs() -> u64 {
    // 单测把时钟钉住，才能确定性地走到「静默 2s 后到期」那一帧；生产构建里
    // 这段完全不存在，仍然只读墙钟。
    #[cfg(test)]
    if let Some(fixed) = TEST_NOW.with(|cell| cell.get()) {
        return fixed;
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// 单测注入的假时钟（`None` = 回到墙钟）。thread_local 让并行测试互不串味。
#[cfg(test)]
thread_local! {
    pub(super) static TEST_NOW: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// 测试时钟的开关。只在 `#[cfg(test)]` 下存在，生产路径无从调用。
#[cfg(test)]
pub(super) fn set_test_now(now: Option<u64>) {
    TEST_NOW.with(|cell| cell.set(now));
}

/// 用系统默认程序打开 hyperlink。失败只留一条日志，绝不 panic（gate 4p）。
/// opener 作为参数是为了让单测能用可控的假实现覆盖「调用路径 + 失败静默」。
fn open_link_with(url: &str, opener: impl FnOnce(&Path) -> std::io::Result<()>) {
    if let Err(err) = opener(Path::new(url)) {
        eprintln!("触发器 Open hyperlink 失败：{err}");
    }
}

/// 一次粘贴实际写出去的东西。
///
/// `bytes` 是交给 PTY 的那一串（已含括号粘贴包装），`lines` 是它顺手完成的
/// 命令行。分成两个字段而不是让调用方自己再算一遍，是为了让「编码」与「历史」
/// 永远是同一次写出的两半。
pub(in crate::ui) struct PasteWrite {
    pub(in crate::ui) bytes: Vec<u8>,
    pub(in crate::ui) lines: Vec<String>,
}

impl Tab {
    /// 这个 tab 还挂着会话没有。等价于旧的 `tab.handle.is_some()`。
    pub(in crate::ui) fn is_live(&self) -> bool {
        self.session.is_some()
    }

    /// 会话 id（夹具与日志文件名要用）；断开后 `None`。
    pub(in crate::ui) fn session(&self) -> Option<SessionId> {
        self.session
    }

    /// 取走「刚刚连上」的边沿。**消费即清**：窗口层每帧都问，问过就不再算数，
    /// 否则关掉自动日志再打开会捡到一条很久以前的旧边沿，凭空给一条老会话开日志。
    pub(in crate::ui) fn take_just_connected(&mut self) -> bool {
        std::mem::take(&mut self.just_connected)
    }

    /// 事件泵：把本帧到达的事件并进快照，返回**是否有新事实**。
    ///
    /// 只有 `true` 时调用方才该重绘。顺带刷新的两样（状态 / 日志态）是
    /// **事件**报上来的——控制面把它们记在会话上，这里不额外发命令。
    ///
    /// `rules` 是全局触发器规则（来自 `Prefs`，经窗口缓存），`run_commands` 收集
    /// 本轮命中里 `RunCommand` 动作要派发的命令 ID——真正的 `dispatch` 在窗口层
    /// （`RunCommand` 走既有命令通道，需要窗口级的 `Action` 分发器，本 tab 没有）。
    /// `poll` 的旧签名：给没有窗口提示槽的调用方（夹具 / 工具路径）用。
    /// 错误不会静默吞掉——没有状态行可写时至少落到 stderr。
    pub(in crate::ui) fn poll(
        &mut self,
        rules: &[Rule],
        run_commands: &mut Vec<String>,
        auto_reconnect: bool,
        onekeys: &[term_core::onekey::OneKey],
        autofill: &std::collections::HashMap<String, String>,
    ) -> bool {
        let mut note = None;
        let changed = self.poll_with_note(rules, run_commands, auto_reconnect, onekeys, autofill, &mut note);
        if let Some(note) = note {
            eprintln!("{note}");
        }
        changed
    }

    /// 与 [`poll`](Self::poll) 同一份泵，额外把触发器副作用错误交给窗口的
    /// 提示槽（状态行）。窗口每帧把 `&mut self.log_note` 传进来。
    pub(in crate::ui) fn poll_with_note(
        &mut self,
        rules: &[Rule],
        run_commands: &mut Vec<String>,
        auto_reconnect: bool,
        onekeys: &[term_core::onekey::OneKey],
        autofill: &std::collections::HashMap<String, String>,
        note: &mut Option<String>,
    ) -> bool {
        let now = now_secs();
        // 2026-11-19 只读查看：**先**用「上一帧的 history + 当前 scroll」算出用户
        // 此刻看到的顶部绝对行，末尾再按新的 history 钉回去。
        //
        // 为什么在开头取：`scroll` 是相对贴底的位移，而下面会把 `snapshot` 换成
        // 新快照（history 变大）。等换完再算，同一个 `scroll` 已经指向另一段内容，
        // 画面就会自己往上爬——那正是「只读还在自动滚动」这个 bug 的成因。
        // 取在这里不引入新字段：每帧都能从既有两份状态推出来，多存一份就是
        // 第二个写者（本仓对这类镜像状态一贯的态度）。
        let held_top = self
            .local_mode
            .then(|| self.snapshot.history as i64 - self.viewport.scroll());
        // 规则清单可能随设置页保存而变化。用 id+enabled+pattern 的粗略
        // 指纹判断是否需要重转，避免每帧重新编译正则（教训）。影子轨在
        // 首帧装入，之后只在指纹变化时 reload。
        let fingerprint = crate::automation::sources::rules_fingerprint(rules);
        if self.shadow.signature() != fingerprint {
            let converted = crate::automation::sources::convert_rules(rules);
            self.shadow.reload(&converted);
            self.shadow.set_signature(fingerprint);
        }
        // 先把 tee 里的字节搬进环，**与显示模式无关、与本帧有没有新事实无关**。
        //
        // 为什么只在这一处搬、不在绘制里搬：`pane.rs` 只画可见标签，后台标签
        // 一帧都不会被画，而 tap 是无界 channel（监管者那边 `send` 不许阻塞），
        // 不搬就是一条**静默的内存泄漏**；更糟的是切到 Bin 时历史会缺掉「没被
        // 画过」的那几段，第一屏又变成从切换那一刻开始——那正是本轮要消灭的 bug。
        // 所以：谁每帧都被调用，谁负责搬。`draw()` 对**所有**标签调一次 `poll`。
        //
        // 搬环的同时把本帧字节带回来，顺手喂本 tab 的 tmux 控制模式。
        // 普通会话的行全是 Passthrough（不置 `active`），因此后台标签 / 非 tmux
        // 会话全量喂也安全——只有真 `%begin` 一类控制行才把 `tmux.active` 置位。
        // 用 owned 的 `Vec<u8>` 而不是回调：闭包里同时借 `&mut self.raw` 与
        // `&mut self` 会打架，分两步就没有重叠的借用。
        let bytes = self.raw.drain_take();
        let moved = !bytes.is_empty();
        if moved {
            // 2026-11-19 只读查看：输出照收（画面要更新），但**触发器一条都不求值**
            // ——`SendText` / `SendLine` / `Beep` / `RunCommand` 都是副作用，只读
            // 态下它们等于「用户没动，程序自己在往终端里写字」。
            //
            // tmux 控制行要照喂：它是**解析**，把 `%begin` 一类行从显示流里摘出来，
            // 不是副作用；跳过它反而会让控制行混进屏幕。
            if self.local_mode {
                self.tmux_consume_bytes(&bytes);
            } else {
                // 输出是触发器的主事件源。先把本帧字节喂给求值器（utf8 宽松解码：
                // 控制序列里夹杂的非 UTF-8 字节不该让一条规则漏触发），命中的动作立即
                // 落地——`SendText` / `SendLine` / `Beep` 走 `write`（PTY 唯一漏斗），
                // `RunCommand` 收集到 `run_commands` 交给窗口层派发。
                let text = String::from_utf8_lossy(&bytes);
                // Modem 自动触发：与触发器读同一份本帧输出，但只认提示符后 /
                // 行首的 rz/sz/rx/rb。命中只记在 tab 上，落地在窗口级 modem_auto。
                self.modem.note(&text, &self.config.modem_commands);
                let target = term_core::trigger_target::Target {
                    system: &self.config.system,
                    session: &self.config.name,
                };
                let hits = self.triggers.feed_for(rules, &text, now, Some(target));
                // Send Condition / Send Later 命中先排队；立即放行的原样返回。
                let mut hits = self.triggers.schedule(hits, rules, now);
                if text.contains('\n') {
                    hits.extend(self.triggers.on_line_break(now));
                }
                if term_app::trigger_builtins::looks_like_prompt(&text) {
                    hits.extend(self.triggers.on_prompt(now));
                }
                hits.extend(self.triggers.due(now));
                // 影子轨用同一批文本求值。先记旧轨命中的规则 id（hit 随后被
                // apply_trigger 消费），再把信号喂给新内核；不执行新轨的动作。
                let v1: Vec<String> = hits.iter().map(|h| h.rule_id.clone()).collect();
                let now_millis = crate::automation::now_ms();
                self.shadow.note_output(now_millis);
                let sig = crate::automation::sources::output_signal("", &text, now_millis);
                self.shadow_check(sig, &v1);
                for hit in hits {
                    self.apply_trigger(hit, run_commands, note);
                }
                // Account OneKey 的 expect → send：同一个提示只回一次，
                // 方案名写错 / 已停用时 `active()` 返回 None，静默跳过。
                if let Some(scheme) = term_core::onekey::OneKey::active(onekeys, &self.config.onekey) {
                    let expects = scheme.expects.clone();
                    for expect in &expects {
                        if self.onekey_expects_fired.contains(&expect.expect) {
                            continue;
                        }
                        if term_core::onekey::expect_matches(expect, &text) {
                            self.onekey_expects_fired.insert(expect.expect.clone());
                            // Password 组的 Send 不在 prefs（明文），应答从保险库
                            // 派生的表里取；空串说明没存过，不静默发空密码。
                            let answer = if expect.send.is_empty() {
                                autofill.get(&scheme.name).map(String::as_str).unwrap_or("")
                            } else {
                                expect.send.as_str()
                            };
                            if !answer.is_empty() {
                                self.write(answer.as_bytes());
                            }
                        }
                    }
                }
                self.tmux_consume_bytes(&bytes);
            }
        }
        // 会话级事件（Connect / Disconnect / Idle）。用 `was_live` 检测跳变：
        // 连上瞬间 reset 求值器（清掉上一次会话的尾巴 / 冷却 / 一次性标志），
        // 让 Connect 重新触发；断开瞬间发 Disconnect，下次重连再起一轮。
        //
        // 只读查看时整段跳过（不求值 = 不执行），但 `was_live` 照常更新——否则
        // 退出只读的第一帧会把「连上」当成一次新跳变，补跑一遍 Connect 动作。
        let live = self.session.is_some();
        if self.local_mode {
            self.was_live = live;
        } else if live && !self.was_live {
            // 「刚连上」的边沿：窗口层的自动日志（`Log types`）只认这一处。
            // 重连会换一个新 `SessionId`，`was_live` 那时已是 false，所以边沿
            // 会再来一次——这正是重连后要重新开一份日志的原因。
            self.just_connected = true;
            // 连上就把退避清零：下次掉线从 1 秒重新起算。
            self.auto_reconnect_at = None;
            self.auto_reconnect_attempts = 0;
            self.triggers.reset();
            self.shadow.reset();
            self.onekey_expects_fired.clear();
            let hits = self.triggers.event(rules, TriggerOn::Connect, now);
            let v1: Vec<String> = hits.iter().map(|h| h.rule_id.clone()).collect();
            let sig = crate::automation::sources::connected_signal("", crate::automation::now_ms());
            self.shadow_check(sig, &v1);
            for hit in hits {
                self.apply_trigger(hit, run_commands, note);
            }
        } else if !live && self.was_live {
            // 断开边沿清掉这条会话的运行时凭据：Tab.auth 里只剩引用，没有
            // 密码就再也取不到——凭据不再活完整场会话。重连会重新问保险库。
            crate::ui::dialogs::vault::clear_session_secrets(&self.config.name);
            let hits = self.triggers.event(rules, TriggerOn::Disconnect, now);
            let v1: Vec<String> = hits.iter().map(|h| h.rule_id.clone()).collect();
            let sig = crate::automation::sources::disconnected_signal("", crate::automation::now_ms());
            self.shadow_check(sig, &v1);
            for hit in hits {
                self.apply_trigger(hit, run_commands, note);
            }
            // 断连边沿：排队中的 Send Condition / Send Later 显式取走并丢弃。
            // 会话已不在，这些发送落不了地；静默留着会在下一条会话上诈尸。
            self.discard_pending_sends(now, note);
            // 掉线边沿：显式 Disconnect / 本地 shell / 已达上限都不排；
            // 其余按 1/2/4/8 秒退避，尝试次数在连上时清零。
            if auto_reconnect
                && !self.auto_reconnect_blocked
                && self.config.protocol != term_core::session::Protocol::Shell
            {
                if let Some(delay) = super::super::reconnect::backoff(self.auto_reconnect_attempts) {
                    self.auto_reconnect_at = Some(now + delay);
                    self.auto_reconnect_attempts += 1;
                }
            }
        }
        // 只有「只读查看」才整段跳过；不能拿 `live &&` 把 `due` 一起关掉——
        // 会话断掉之后没有输出事件，Send Later 本来就该在这条安静期里到期。
        if !self.local_mode {
            if live {
                // Idle 只在「距上次输出至少 idle_secs 秒、且本段空闲还没报过」时命中；
                // `feed` 一旦有输出就把 `idle_fired` 清空，下一段空闲重新开始计。
                let hits = self.triggers.event(rules, TriggerOn::Idle, now);
                let v1: Vec<String> = hits.iter().map(|h| h.rule_id.clone()).collect();
                let at = crate::automation::now_ms();
                let quiet = crate::automation::quiet_since(self.shadow.last_output(), at);
                let sig = crate::automation::sources::idle_signal("", quiet, at);
                self.shadow_check(sig, &v1);
                for hit in hits {
                    self.apply_trigger(hit, run_commands, note);
                }
            }
            // Send Later 的到期只靠输出事件驱动是不够的：远端在命中之后安静下来，
            // 就再也不会有 `on_line_break` / `on_prompt` / `due` 的机会，定时发送
            // 永远不落地。空闲段每帧补一次 `due`（断开之后也一样）。
            // `moved` 那一帧已经在输出分支调过 `due`（它会取走到期项），这里跳过，
            // 避免同一帧把同一条命中发两遍。
            if !moved {
                let due = self.triggers.due(now);
                for hit in due {
                    self.apply_trigger(hit, run_commands, note);
                }
            }
        }
        self.was_live = live;
        let Some(id) = self.session else {
            return moved;
        };
        // ADR-0025：**外部关掉的会话必须在这里被认出来**。
        //
        // `SessionPool::poll` 返回 `None` 只有一种含义：这条 id 已经不在册
        // （`SessionPool::close` 是 `remove`，不是打一个「已关闭」标记）。
        // 不处理的话标签会**永远停在「活着」**：`is_live()` 一直为真 → 标题不出
        // `[exited]`、画面冻结在最后一帧、敲键盘全落空（`write` 对不在册的 id
        // 返回 false）。能造成这种情况的真实路径是脚本 / 桥接的 `session_close`
        // ——它们关得掉用户正看着的那条会话，而本界面完全不知道。
        //
        // 只调**一次** `poll`：它是有副作用的「取走本帧新事实」，调两次会把
        // 第一次的 `true` 吃掉，快照就再也不更新。
        match self.pool.poll(id) {
            None => {
                // 转成既有的「断开」形态（`is_live() == false`）：保留标签与名字，
                // 用户能看见、能手关、能按 `config` 重连。这里**不删标签**——
                // 一条标签在用户眼前凭空消失，比「断开」更难理解。
                self.session = None;
                // 下一帧 `live` 变 false、`was_live` 仍是 true，于是 `Disconnect`
                // 触发器照常命中（那条路本来就是为「会话断开」写的）。
                return true;
            }
            Some(false) => return moved,
            Some(true) => {}
        }
        if let Some(snapshot) = self.pool.snapshot(id) {
            self.snapshot = snapshot;
        }
        // 2026-11-19 只读查看：把视口钉回「进入时看到的那一行」（绝对行）。
        //
        // 新输出让 `history` 变大，`hold_top` 就把 `scroll` 同等抬高，于是同一段
        // 内容仍停在原处——这就是「无自动滚动」。用户自己滚动会改 `scroll`，
        // 下一帧的 `held_top` 从新位移算，所以**想看哪里就看哪里，只是不会被
        // 输出拽走**。
        //
        // 边界：内容还没满一屏时 `hold_top` 只能给 0（贴底），此时画面仍会跟着
        // 新行变——那不是漏了判据，是「没有可回看的历史」时唯一可能的画法。
        if let Some(top) = held_top {
            self.viewport.hold_top(top, self.snapshot.history as i64);
        }
        // 只有真拿到新窗口才覆盖：范围请求与快照是两条独立的事实，
        // 快照更新不代表这一屏回看有新局面（旧代码同样是 `if screen.is_some()`）。
        if let Some(window) = self.pool.screen_window(id) {
            self.screen = Some(window);
        }
        // 预览窗口从独立通道取，与 `screen` 物理隔离（ADR-0028）。
        // 不在这里做「没拿到就清空」：断开会话时预览槽保留上一帧即可，避免闪动。
        if let Some(window) = self.pool.preview_window(id) {
            self.preview_screen = Some(window);
        }
        true
    }

    /// 把一条信号喂给新内核求值，只做影子比对 / 诊断，**不执行任何动作**。
    ///
    /// 影子轨与旧轨并行：`v1` 是旧 `Matcher` 本轮命中的规则 id（由调用方填），
    /// `v2` 是新内核产出的。不一致时累计计数（诊断用，不进持久化、不影响画面）。
    /// 这一步永远运行——即使新轨开关在场，也不从影子轨执行副作用。
    fn shadow_check(&mut self, signal: term_core::automation::Signal<'_>, v1: &[String]) {
        let session_name = self
            .session
            .and_then(|id| self.pool.name(id))
            .unwrap_or_default();
        let target = crate::automation::target_for(&session_name);
        let effects = self.shadow.eval(&signal, &target);
        let report = crate::automation::ShadowReport {
            v2_rules: crate::automation::effect_rule_ids(&effects),
            v1_rules: v1.to_vec(),
        };
        if !report.agrees() {
            self.shadow.note_disagreement();
        }
    }

    /// 把一次命中的动作落到会话上。
    ///
    /// - `SendText` / `SendLine`：经 `Tab::write`（PTY 唯一漏斗）。`SendLine` 补一个
    ///   CR，等价于敲回车；远端行规把 CR 当行结束，与键盘 Enter 一致。
    /// - `Beep`：写终端响铃字符 `BEL (0x07)` 到 PTY——这是本层能做到的「真实副作用」
    ///   （本地桌面响铃需要 OS / ctx，tab 不持有；v0.3 若有更合适的提醒通道再换）。
    /// - `RunCommand`：收集命令 ID，交给窗口层 `dispatch`——它走既有命令通道，
    ///   需要 `Action` 分发器，tab 这一层没有。
    /// - `Highlight`：记下命中所在的内容行，`paint_rows` 在字形下铺一层淡底；
    ///   不进选区、不影响复制。
    /// - `OpenLink`：经投影层 `sysopen::open` 用系统默认程序打开；失败只记日志。
    pub(in crate::ui) fn apply_trigger(
        &mut self,
        hit: term_app::trigger::Hit,
        run_commands: &mut Vec<String>,
        note: &mut Option<String>,
    ) {
        for action in &hit.actions {
            match action {
                // 只有文本里**明确写了转义**才走 Sender 那张表：`\x03` / `\003`
                // / `\cc` 都要能发 Ctrl+C。普通文本（例如 `C:\Windows\path`）
                // 原样发，不能被未知转义吃掉反斜杠，也不能因末尾孤立 `\` 整条不发送。
                // 写错的转义不回退成字面字节：既不发，也把原因送到状态行。
                TriggerAction::SendText(s) => match decode_send_text(s) {
                    Ok(bytes) => {
                        self.write(&bytes);
                    }
                    Err(err) => note_escape_error(note, &hit.rule_id, "Send text", &err),
                },
                TriggerAction::SendLine(s) => match decode_send_text(s) {
                    Ok(mut bytes) => {
                        bytes.push(b'\r');
                        self.write(&bytes);
                    }
                    Err(err) => note_escape_error(note, &hit.rule_id, "Send line", &err),
                },
                TriggerAction::Beep => {
                    self.write(&[0x07]);
                }
                TriggerAction::RunCommand(id) => {
                    run_commands.push(id.clone());
                }
                TriggerAction::OpenLink(url) => open_link_with(url, crate::sysopen::open),
                TriggerAction::LocalSendLine(s) => {
                    let mut text = s.clone();
                    text.push('\r');
                    match term_core::send_escapes::decode(&text) {
                        Ok(bytes) => self.pending_local_sends.push(String::from_utf8_lossy(&bytes).into_owned()),
                        Err(err) => note_escape_error(note, &hit.rule_id, "Local send line", &err),
                    }
                }
                TriggerAction::Highlight => {
                    // 命中发生在这一帧的事实窗口里：记下光标的绝对内容行，
                    // 由 paint_rows 铺一层淡底。只保留最近 64 行，避免长会话
                    // 把高亮表养大；复制 / 选区完全不受影响。
                    if let Some((row, _)) = self.view.cursor {
                        let row = super::highlights::absolute_row(row, self.view.range.start);
                        if !self.trigger_highlight_rows.contains(&row) {
                            self.trigger_highlight_rows.push(row);
                            if self.trigger_highlight_rows.len() > 64 {
                                self.trigger_highlight_rows.remove(0);
                            }
                        }
                    }
                }
            }
        }
    }

    /// 断连边沿把排队的发送意图取走并丢弃，同时报一条提示。
    ///
    /// `Matcher::reset` 只清尾巴 / 冷却 / 一次性表，不清 trigger queue；会话
    /// 已经不在，这些命中发不出去，留到下一次连接就会变成一条迟到的发送。
    /// 这里用公开的事件 / 时钟入口把队列抽干，一条都不应用，并让用户知道
    /// 「刚才排队的没送出去」。
    pub(in crate::ui) fn discard_pending_sends(&mut self, now: u64, note: &mut Option<String>) {
        let mut dropped = self.triggers.on_prompt(now).len();
        dropped += self.triggers.on_line_break(now).len();
        // `u64::MAX` 让所有 Send Later 都算到期，连未到期的也一并丢弃。
        dropped += self.triggers.due(u64::MAX).len();
        if dropped > 0 && note.is_none() {
            *note = Some(format!("会话已断开，{dropped} 条待发送触发器已丢弃（未送达）"));
        }
    }

    /// 喂一行本会话收到的 tmux 控制行；普通行是 Passthrough，不吞也不置 active。
    pub(in crate::ui) fn tmux_feed_line(&mut self, line: &str) {
        self.tmux.feed(line);
    }

    /// 把 tap 字节攒进跨块缓冲，按 `\n` 切整行喂给**本 tab** 的 ControlMode。
    ///
    /// 控制行可能被任意切成块（`%be` / `gin …\n`），所以半行留在
    /// `tmux_pending` 里等下一块；空行跳过；行尾的 `\r` 一并剥掉，免得
    /// `%begin` 后面那个 CR 把指令名粘成 `%begin\r`。
    pub(in crate::ui) fn tmux_consume_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.tmux_pending.extend_from_slice(bytes);
        while let Some(pos) = self.tmux_pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.tmux_pending.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\r', '\n']);
            if !line.is_empty() {
                self.tmux_feed_line(line);
            }
        }
    }

    /// 挂上字节 tee（非破坏性复制：VT 照吃、屏幕照常前进，与 `attach_raw` 相反）。
    ///
    /// 无界 channel 是**故意**的：监管者线程侧 `send` 绝不许阻塞，UI 卡一下不能
    /// 把会话线程一起拖住。代价是「UI 不搬就攒着」，由 [`Tab::poll`] 每帧搬。
    pub(in crate::ui) fn attach_tap(&mut self) {
        let Some(id) = self.session else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        // 先入队、后 ack 的语义在控制面里（见 `SessionPool::attach_tap`）；
        // 这里只认结果：挂上了才把接收端留在环上，否则留一个永远空的 rx。
        // 接收端包成 `HexTap`：`Receiver` 本身不进 `ui/`（见 `hex_tap.rs`）。
        if self.pool.attach_tap(id, tx) {
            self.raw.attach(crate::hex_tap::HexTap::new(rx));
        }
    }

    /// 摘掉 tee。**环不清**：断开之后用户还该能看断开前收到的那一段，
    /// 清掉等于「一断开就什么都不剩」。
    pub(in crate::ui) fn detach_tap(&mut self) {
        if let Some(id) = self.session {
            self.pool.detach_tap(id);
        }
        self.raw.detach();
    }

    /// 往会话写字节（键盘 / 粘贴 / 设备应答）。返回是否真的送到了。
    ///
    /// `Text ▸ Hide cursor when typing`：这里顺手记下输入时刻——它是所有写
    /// PTY 路径的唯一漏斗（见 `Tab::last_input_at`）。**不必**送到才算：没连上
    /// 的标签敲键盘同样是「用户正在打字」，光标那一下藏不藏与送达无关。
    ///
    /// 2026-11-19 **只读查看**（`Window ▸ Local Mode`）：这里直接返回 `false`，
    /// 一个字节都不发，也**不**记输入时刻（只读时不算打字）。收在这一处是因为
    /// 它是唯一漏斗：键盘、粘贴、鼠标上报、工具面板、快捷命令、触发器动作、
    /// 响铃全都在下面，拦一处等于全拦——九处各写一遍判断迟早漏一处。
    pub(in crate::ui) fn write(&self, bytes: &[u8]) -> bool {
        if self.local_mode {
            return false;
        }
        self.last_input_at.set(Some(std::time::Instant::now()));
        self.session.is_some_and(|id| self.pool.send(id, bytes))
    }

    /// 粘贴的写入：编码 → 写 PTY → 喂输入行，三步只此一份。
    ///
    /// 为什么收成一个方法：粘贴有「直接落地」与「弹窗确认后落地」两条路，
    /// 两条路各拼一次编码，就会出现「同一段文本在主屏与确认后粘出不同的字节」
    /// ——括号粘贴 / 换行归一 / 控制字符过滤三件事只要漏一处，远端就收到
    /// 另一种东西。返回值把**真正写出去的字节**交出来，单测据此把两条路
    /// 逐字节对齐（`pane.rs` 的 `confirming_does_not_change_the_bytes`）。
    pub(in crate::ui) fn write_paste(&mut self, text: &str) -> PasteWrite {
        // 只读查看：不编码、不写、不喂输入行。返回空结果——调用方据此既不
        // 追加历史行，也不会在折叠表里凭空记出一段「粘过的文本」。
        if self.local_mode {
            return PasteWrite {
                bytes: Vec::new(),
                lines: Vec::new(),
            };
        }
        // 编码在语义平面（换行归一 / 控制字符过滤 / 括号粘贴），
        // 投影只提供「此刻终端处于哪种模式」——它手上只有快照。
        let bytes = paste_bytes_for(text, self.snapshot.bracketed_paste).into_bytes();
        self.write(&bytes);
        // 粘贴同样进输入行（多行粘贴会吐多行）。
        let lines = self.input_line.feed(&bytes);
        // 折叠的输入行表：粘贴的这几行从光标行往下数（回显还没到，光标仍在
        // 粘贴段的第一行上，见 `Tab::note_pasted_lines`）。
        self.note_pasted_lines(lines.len());
        PasteWrite { bytes, lines }
    }

    /// 调整终端尺寸。控制面内部按尺寸去重，每帧调用也安全。
    pub(in crate::ui) fn resize(&self, cols: usize, rows: usize) {
        if let Some(id) = self.session {
            self.pool.resize(id, cols, rows);
        }
    }

    /// 请求一段回看窗口；结果下一帧从 [`screen_window`](Tab::screen_window) 取。
    ///
    /// 收 [`ViewRange`] 而不是裸 `Range<i64>`：调用方说的是视口行，这个语义一直
    /// 留到 `SessionPool`——之后这条链上不再有 `to_i64()`，擦除只发生在 core
    /// 内部（`slice_view` / `crop_view` / `blank_view` 各自的委托处）。
    pub(in crate::ui) fn request_range(&self, range: ViewRange) {
        if let Some(id) = self.session {
            self.pool.request_range(id, range);
        }
    }

    /// 上一次范围请求拿到的事实窗口。
    pub(in crate::ui) fn screen_window(&self) -> Option<ScreenImage> {
        self.session.and_then(|id| self.pool.screen_window(id))
    }

    /// 请求一份预览用事实窗口（悬停滚动条时的历史快照）。第二通道，
    /// 与 [`request_range`](Tab::request_range) 物理隔离（ADR-0028）。
    pub(in crate::ui) fn request_preview_range(&self, range: ViewRange) {
        if let Some(id) = self.session {
            self.pool.request_preview_range(id, range);
        }
    }

    /// 上一次预览范围请求拿到的事实窗口（独立槽）。
    pub(in crate::ui) fn preview_window(&self) -> Option<ScreenImage> {
        self.session.and_then(|id| self.pool.preview_window(id))
    }

    /// 运行时状态（连接 / 存活 / 退出文本）。断开后给一份诚实的默认值：
    /// 没连上、没活着、也没有退出文本——不是「假装还连着」。
    pub(in crate::ui) fn status(&self) -> SessionStatus {
        self.session
            .and_then(|id| self.pool.status(id))
            .unwrap_or_default()
    }

    /// 清屏（菜单 View › Clear / 工具栏那个按钮）。
    pub(in crate::ui) fn clear(&self) {
        if let Some(id) = self.session {
            self.pool.clear(id);
        }
    }

    /// 只丢历史（右键 `Clear Scrollback`）。屏幕不动。
    pub(in crate::ui) fn clear_scrollback(&self) {
        if let Some(id) = self.session {
            self.pool.clear_scrollback(id);
        }
    }

    /// 清屏 + 清历史（右键 `Clear All`）。
    pub(in crate::ui) fn clear_all(&self) {
        if let Some(id) = self.session {
            self.pool.clear_all(id);
        }
    }

    /// 会话日志的当前状态。断开或没开会话 → `Off`。
    pub(in crate::ui) fn log_status(&self) -> LogStatus {
        self.session
            .and_then(|id| self.pool.log_status(id))
            .unwrap_or_default()
    }

    /// 开始记录到 `dir` 下。文件名 / 行格式 / 覆盖方式 / 大小上限由 `opts` 定
    /// （来源是 `Session Settings ▸ Logging` 那五个字段）。
    pub(in crate::ui) fn log_start(
        &self,
        dir: PathBuf,
        session_name: &str,
        opts: &term_core::log_opts::LogOptions,
    ) {
        if let Some(id) = self.session {
            self.pool.start_log(id, dir, session_name, opts);
        }
    }

    pub(in crate::ui) fn log_stop(&self) {
        if let Some(id) = self.session {
            self.pool.stop_log(id);
        }
    }

    pub(in crate::ui) fn log_pause(&self) {
        if let Some(id) = self.session {
            self.pool.pause_log(id);
        }
    }

    pub(in crate::ui) fn log_resume(&self) {
        if let Some(id) = self.session {
            self.pool.resume_log(id);
        }
    }

    /// 注入唤醒回调：有输出就请框架重绘。
    ///
    /// 与旧代码同一个理由：`draw()` 只在有输入事件时才跑，没有这个回调，
    /// PTY 的输出到了也没人叫醒界面。
    pub(in crate::ui) fn set_wakeup(&self, ctx: &egui::Context) {
        if let Some(id) = self.session {
            let ctx = ctx.clone();
            self.pool
                .set_wakeup(id, std::sync::Arc::new(move || ctx.request_repaint()));
        }
    }

    /// 关掉会话并断开这个 tab。已断开时是空操作（不是错误）。
    ///
    /// 关键差别：**关会话必须经控制面**。旧代码是丢掉 `SupervisorHandle` 让
    /// 它的 `Drop` 去发 Close——句柄只在 UI 手上时那样成立；现在句柄在池里，
    /// 丢一个 `SessionId` 什么都不会发生。
    pub(in crate::ui) fn close(&mut self) {
        // 先摘 tee 再关会话：反过来的话 `session` 已经被 take 走，`detach_tap`
        // 找不到会话可摘，监管者那边要等它自己发现 sink 断了才清槽位。
        self.detach_tap();
        if let Some(id) = self.session.take() {
            self.pool.close(id);
        }
    }
}

/// Trigger ▸ Send text 的解码口径。
///
/// 与 Sender 共用转义表是原版口径，但那张表对**未知转义**会吞掉反斜杠
/// （`\q` → `q`），末尾孤立 `\` 还会直接报错导致整条不发送。对 Windows 路径
/// 这种「用户本来就想发字面反斜杠」的文本，那是静默的数据损坏。所以只有
/// 文本里确实出现了表内的显式转义时才解码；其余一律按字面字节发送。
///
/// 第三轮：显式转义写错时返回 `Err`，由调用方上报到状态行，**不再静默回退**
/// 成原样字节（那等于发了一串反斜杠，比不发更糟）。
pub(super) fn decode_send_text(
    text: &str,
) -> Result<Vec<u8>, term_core::send_escapes::EscapeError> {
    let mut chars = text.chars().peekable();
    let mut explicit = false;
    while let Some(c) = chars.next() {
        if c == '\\'
            && chars
                .peek()
                .is_some_and(|next| matches!(next, '0'..='7' | 'a' | 'b' | 't' | 'n' | 'v' | 'f' | 'r' | '\\' | 'x' | 'u' | 'c' | 'm'))
        {
            explicit = true;
            break;
        }
    }
    if explicit {
        return term_core::send_escapes::decode(text);
    }
    Ok(text.as_bytes().to_vec())
}

/// 一处转义写错：把「哪条规则的哪个动作」与错误原因写进窗口提示槽。
/// 同一批动作里只留第一条，避免一次命中把状态行刷成同一句话的列表。
fn note_escape_error(
    note: &mut Option<String>,
    rule: &str,
    action: &str,
    err: &term_core::send_escapes::EscapeError,
) {
    if note.is_none() {
        *note = Some(format!("触发器 {rule} 的 {action} 转义写错：{err}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// `apply_trigger` 的 `OpenLink` 分支经这个辅助调用 opener；这里用假 opener
    /// 钉住「URL 原样传出」与「失败不 panic」两件事，不在桌面里真起进程。
    #[test]
    fn open_link_calls_the_opener_with_the_exact_url() {
        let seen = RefCell::new(Vec::new());
        open_link_with("https://e.test/a b", |path| {
            seen.borrow_mut().push(path.to_path_buf());
            Ok(())
        });
        assert_eq!(seen.borrow().as_slice(), &[PathBuf::from("https://e.test/a b")]);
    }

    #[test]
    fn open_link_swallows_opener_errors() {
        let called = RefCell::new(false);
        open_link_with("missing", |_| {
            *called.borrow_mut() = true;
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no opener"))
        });
        assert!(*called.borrow());
    }
}
