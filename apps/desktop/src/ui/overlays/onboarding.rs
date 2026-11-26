//! 首次启动引导的绘制层（`docs/design/09-session-import-and-onboarding.md`）。
//!
//! 两页：
//!
//! - **Welcome**：一两句说明 + `Next` / `Skip`。
//! - **Detect**：探测到的 SSH 主机清单（勾选框 + 会话名 + `user@host:port` +
//!   来源文件）+ `Back` / `Import N sessions` / `Skip`。一台都没探到就画一行说明
//!   加 `Finish`。
//!
//! # 这一层只有三件真事
//!
//! 1. **读文件 + 摊成主机**：`~/.ssh/config` 与 WindTerm 的 `profiles/`。与
//!    `ui/panels/sidebar.rs::import_text` 同一条边界——那边也只负责「找到文件」，
//!    解析复用全仓唯一那份 ssh config 解析器；本文件只把解析结果摊成
//!    `DetectedHost`（多一个 `source` 字段），不写第二个解析器。
//! 2. **画**（`StableWindow` + 固定矩形，与其余对话框同一套首帧即最终布局）。
//! 3. **落地**：勾上的主机 → `SessionConfig`（追加进 `self.sessions`）→ 落盘 →
//!    写 `prefs.onboarded = true`。走完与跳过**共用同一条收尾路径**
//!    （`finish_onboarding`），因为「用户看过了」是这两件事唯一的共同点。
//!
//! # 为什么不把状态放在窗口上
//!
//! 页号 / 勾选位 / 提示语全在 `term_app::onboarding::OnboardingState`（纯逻辑，
//! 8 条单测留在 term-app）。「文本 → 主机」的解析适配在本文件完成（11 条单测在
//! `onboarding_tests.rs`）：投影层本来就要读文件，把薄适配器放在这里，无前端
//! 应用层便不必为一个转手函数拖进带 IO 的 crate。本文件只持有一份
//! `self.onboarding` 副本，绘制期间**只读**它，改动以 [`Intent`] 出队、闭包外
//! 统一施加——与命令面板 / Goto 同一套受控契约。

use crate::ui::dialogs::{save_prefs, unique_name};
use crate::ui::kit::window::{DialogId, DialogWindow};
use crate::ui::*;
use term_app::i18n::Lang;
use term_app::onboarding::{DetectedHost, OnboardingState, Step};
// 全仓唯一那份 ssh config / WindTerm profiles 解析器。desk 已直接依赖
// `term-runtime`，其 `import_sessions` 把解析层整个再导出（`sidebar.rs` 的
// `Import ▸ OpenSSH` 走同一条路径）。放在投影层而不是 `term-app`：后者要保住
// 「依赖表只有 term-core」这条编译期边界（ADR-0016）。
use term_runtime::import_sessions::{
    parse_ssh_config as parse_ssh, parse_windterm, ImportedSession,
};
// 上游 `user.sessions` 的扁平 JSON 走 term-import 的新解析器：它按协议拆 target，
// 不会像旧的宽松解析器那样把 9 条都读成 0 条。
use term_import::sessions::parse_windterm_sessions;

/// 钉死的矩形（画布 1280×760 里居中）：600×380。
///
/// 与 `dialogs/quick_commands.rs` 同一个做法——`resizable(false)` 的窗口仍会随
/// 内容伸缩，而截图在第 2 pass 就退出；首帧即最终矩形才可复现。这里选 600 宽是
/// 因为清单行里有 `user@host:port` 与来源路径两段文本，再窄就要提前省略。
/// 主机清单的可视高度：超出进滚动区，不让清单把按钮行顶出画布。
const LIST_H: f32 = 178.0;
/// 清单每行的高度（勾选框 + 两行文本的紧凑布局）。
const ROW_H: f32 = 34.0;

/// 导入进来的会话落在哪个分组。
///
/// 用一个**新**分组而不是塞进 `Servers`：引导导入的是「这台机器上原本就有的
/// 东西」，与用户自己建的会话混在一棵树里，第一次启动看到的就是一团（PRD 要的是
/// 「预填会话树」，不是「替你建国」）。分组名是**数据**不是 chrome，不进词表
/// （见 `term_app::i18n` 模块文档：会话名 / 分组名 / 终端输出都不是界面文案）。
const IMPORT_GROUP: &str = "Imported";

/// 绘制期间收集的意图。闭包外统一施加——绘制时不写领域状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Intent {
    Next,
    Back,
    Skip,
    Import,
    /// `Finish`：只有「一台都没探到」那一页有它，语义等同关闭。
    Finish,
    /// 勾 / 取消第 `usize` 条。
    Toggle(usize),
}

impl TerminalWindow {
    /// 画引导。默认关闭时一行都不画（与其余对话框一致）。
    pub(in crate::ui) fn draw_onboarding(&mut self, ctx: &egui::Context) {
        if !self.view.show_onboarding {
            return;
        }
        // `Done` 不是一页：它表示「走完 / 跳过」，投影层见到它就收尾。
        // 放在画之前判，于是窗口不会有一帧空壳。
        if self.onboarding.step == Step::Done {
            self.finish_onboarding();
            return;
        }

        let lang = self.view.lang;
        let state = &self.onboarding;
        let mut intent: Option<Intent> = None;
        // 标题跟着页走，但**窗口 id 固定**（`harness::StableWindow` 的规矩：
        // 两页共用一个窗口，用标题生成 id 会让它们各存一份布局状态）。
        let title = match state.step {
            Step::Welcome => t(lang, "Welcome"),
            Step::Detect | Step::Done => t(lang, "Detected hosts"),
        };
        let open = DialogWindow::new(DialogId::Onboarding, title).show(
            ctx,
            |ui| match state.step {
                Step::Welcome => draw_welcome(ui, lang, &mut intent),
                Step::Detect => draw_detect(ui, lang, state, &mut intent),
                // 上面已经拦掉了，这里只是把「三态」写全，避免将来加页时漏一支。
                Step::Done => {}
            },
        );

        match intent {
            Some(Intent::Next) => self.onboarding.next(),
            Some(Intent::Back) => self.onboarding.back(),
            Some(Intent::Skip) | Some(Intent::Finish) => self.onboarding.skip(),
            Some(Intent::Import) => {
                self.import_detected();
                self.onboarding.skip();
            }
            Some(Intent::Toggle(index)) => self.onboarding.toggle(index),
            None => {}
        }
        // 右上角 × 与 `Skip` 走同一个结局：用户不想看第二遍。
        if !open {
            self.onboarding.skip();
        }
        // 一次收敛所有「已经走到 Done」的路径（next 越过 Detect / skip / ×）。
        if self.onboarding.step == Step::Done {
            self.finish_onboarding();
        }
    }

    /// 打开引导并做一次探测。**在 `construct.rs::init` 里调用一次**（首帧之前）。
    ///
    /// `self.view.show_onboarding` 与 `self.onboarding.open` 一起置位：两个字段
    /// 描述同一件事（可见性），恒定同步，没有第二个写入者。
    pub(in crate::ui) fn open_onboarding(&mut self) {
        self.onboarding.open();
        self.onboarding.load_detected(detect_hosts());
        self.view.show_onboarding = true;
    }

    /// 收尾：写 `prefs.onboarded = true`、关窗口。
    ///
    /// 走完与跳过都到这里。**必须落盘**：只改内存的话下次启动会再弹一遍，
    /// 那是比不做引导更烦人的形态。
    fn finish_onboarding(&mut self) {
        save_prefs(|p| p.onboarded = true);
        self.onboarding.close();
        self.view.show_onboarding = false;
    }

    /// 把勾选的主机追加进会话树并落盘。
    fn import_detected(&mut self) {
        // 先把要导入的**拷出来**：`take_accepted` 借着 `self.onboarding`，
        // 而下面要可变地改 `self.sessions` / `self.groups`。两段分开，不打架。
        let picked: Vec<DetectedHost> = self
            .onboarding
            .take_accepted()
            .into_iter()
            .cloned()
            .collect();
        let mut added = merge_detected(&mut self.sessions, &mut self.groups, picked);
        // 上游 `user.sessions` 不摊成 DetectedHost：那个结构把协议硬编码成 Ssh，
        // Telnet / Tcp / Shell 会被改错。这里直接走侧栏落库用的 ImportedSession，
        // 由解析器保留协议、用户名与端口（「走侧栏落库」这一支）。
        added += merge_imported(
            &mut self.sessions,
            &mut self.groups,
            detect_windterm_sessions(),
        );
        if added == 0 {
            return;
        }
        // 与侧栏导入同一条收尾：树按 (分组, 名字) 重新排一遍再落盘。
        self.sort_sessions();
        self.save_sessions();
    }
}

/// 把探测到的主机并进会话树，返回新增条数。
///
/// 抽成**纯函数**（只吃两个 `Vec`）是为了能测：真正的那条路最后一步是
/// `save_sessions()`，会写用户真实的会话库，单测不能碰。
fn merge_detected(
    sessions: &mut Vec<SessionConfig>,
    groups: &mut Vec<String>,
    hosts: Vec<DetectedHost>,
) -> usize {
    let mut added = 0;
    for host in hosts {
        // 撞名策略：不覆盖、不跳过、也不留两个同名。名字没被占用时原样用
        // `label`；被占用了才交给既有的 `unique_name`（它产出 `web (copy)` /
        // `web (copy 2)`，与「复制会话」同一套后缀，用户不必学第二种命名规则）。
        let name = if sessions.iter().any(|s| s.name == host.label) {
            unique_name(sessions, &host.label)
        } else {
            host.label.clone()
        };
        let mut cfg = host.into_session(name);
        cfg.group = IMPORT_GROUP.to_string();
        sessions.push(cfg);
        added += 1;
    }
    // 分组只在真的加进来东西时才建：空导入不该在树上留一个空文件夹。
    if added > 0 && !groups.iter().any(|g| g == IMPORT_GROUP) {
        groups.push(IMPORT_GROUP.to_string());
    }
    added
}

/// 把上游 `user.sessions` 的解析结果并进会话树，保留每条自己的协议 / 端口 / 用户。
/// 与 [`merge_detected`] 同一套撞名策略（改名不覆盖），但不去改协议。
fn merge_imported(
    sessions: &mut Vec<SessionConfig>,
    groups: &mut Vec<String>,
    items: Vec<ImportedSession>,
) -> usize {
    let mut added = 0;
    for item in items {
        let name = if sessions.iter().any(|s| s.name == item.name) {
            unique_name(sessions, &item.name)
        } else {
            item.name.clone()
        };
        let group = item.group.clone();
        let mut cfg = item.into_config();
        cfg.name = name;
        sessions.push(cfg);
        if !groups.iter().any(|g| g == &group) {
            groups.push(group);
        }
        added += 1;
    }
    added
}

/// 上游 `user.sessions`：发现层给出的路径只查一次，解析走 `parse_windterm_sessions`。
/// 读不到 / 一条都没有就是「这台机器上没有」，不伪造。
fn detect_windterm_sessions() -> Vec<ImportedSession> {
    let roots = term_import::discovery::candidate_windterm_roots();
    let exports = term_import::discovery::find_profile_exports_in(&roots);
    exports
        .user_sessions
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| parse_windterm_sessions(&text))
        .unwrap_or_default()
}

/// 第一页：说明 + `Next` / `Skip`。
///
/// 页内不另画 heading：窗口标题已经是 `Welcome`，同一个词上下隔 20px 出现两遍
/// 不是设计，是重复。留白交给固定的窗口高度。
fn draw_welcome(ui: &mut egui::Ui, lang: Lang, intent: &mut Option<Intent>) {
    ui.add_space(18.0);
    ui.label(
        t(
            lang,
            "WindTerm can seed your session tree from the SSH hosts already configured on this machine. Passwords are never copied.",
        ),
    );
    ui.add_space(150.0);
    ui.horizontal(|ui| {
        if ui.button(t(lang, "Next")).clicked() {
            *intent = Some(Intent::Next);
        }
        if ui.button(t(lang, "Skip")).clicked() {
            *intent = Some(Intent::Skip);
        }
    });
}

/// 第二页：主机清单 + `Back` / `Import N` / `Skip`；空清单时只有一句说明 + `Finish`。
fn draw_detect(
    ui: &mut egui::Ui,
    lang: Lang,
    state: &OnboardingState,
    intent: &mut Option<Intent>,
) {
    ui.add_space(6.0);
    if let Some(note) = state.note {
        // 探测失败 / 没有可导入项：那句话就画在清单原本的位置上，不弹 toast
        // （引导是一屏的事，用户不该去别处找解释）。
        ui.label(t(lang, note));
        ui.add_space(150.0);
        if ui.button(t(lang, "Finish")).clicked() {
            *intent = Some(Intent::Finish);
        }
        return;
    }

    egui::ScrollArea::vertical()
        .max_height(LIST_H)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, host) in state.detected.iter().enumerate() {
                let mut on = state.accepted.get(index).copied().unwrap_or(false);
                ui.allocate_ui(egui::vec2(ui.available_width(), ROW_H), |ui| {
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut on, "").changed() {
                            *intent = Some(Intent::Toggle(index));
                        }
                        ui.vertical(|ui| {
                            ui.label(&host.label);
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}   {}",
                                    host.target(),
                                    t(lang, "From {path}").replace("{path}", &short_source(&host.source))
                                ))
                                .small()
                                .color(crate::theme::chrome(term_core::theme::CHROME_SCALE.text)),
                            );
                        });
                    });
                });
            }
        });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button(t(lang, "Back")).clicked() {
            *intent = Some(Intent::Back);
        }
        let count = state.accepted_count();
        let import = t(lang, "Import {n} sessions").replace("{n}", &count.to_string());
        if ui
            .add_enabled(count > 0, egui::Button::new(import))
            .clicked()
        {
            *intent = Some(Intent::Import);
        }
        if ui.button(t(lang, "Skip")).clicked() {
            *intent = Some(Intent::Skip);
        }
    });
}

/// 来源路径的展示形态：家目录前缀换成 `~`。
///
/// `~/.ssh/config` 比 `/Users/<名字>/.ssh/config` 短一半，而用户在引导页上只需要
/// 「这些是从哪读来的」这个事实，不需要自己的用户名。其他路径原样返回。
fn short_source(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home = home.to_string_lossy().into_owned();
        if !home.is_empty() {
            if let Some(rest) = path.strip_prefix(&home) {
                return format!("~{rest}");
            }
        }
    }
    path.to_string()
}

/// 别名或 hostname 里能当会话名的那一段。
///
/// `Host web` 的 `web` 就是用户会敲的那串，直接用它；`Host 10.0.0.11` 没有别名，
/// 用 hostname 让树上至少有个能认的名字。两者都空是不可能的（空 `Host` 行会被
/// 解析器当成没有名字的块丢掉），但这里仍然给出一个兜底而不是 `unwrap`。
fn field_label(name: &str, host: &str) -> String {
    if !name.is_empty() {
        name.to_string()
    } else if !host.is_empty() {
        host.to_string()
    } else {
        String::new()
    }
}

/// 把解析器的输出摊成状态机要用的形状。
///
/// 只做**适配**，不写第二个解析器：解析结果已经保证密码 / 口令 / 私钥内容不进来，
/// `DetectedHost` 只是多带一个 `source`（引导页要显示「这些是从哪个文件里读来的」，
/// 侧栏导入路径不需要那句话）。
fn adapt(items: Vec<ImportedSession>, source: &str) -> Vec<DetectedHost> {
    items
        .into_iter()
        .map(|s| DetectedHost {
            label: field_label(&s.name, &s.host),
            host: if s.host.is_empty() { s.name.clone() } else { s.host },
            user: s.username,
            port: s.port,
            source: source.to_string(),
        })
        .collect()
}

/// 解析 `~/.ssh/config` 的文本。
///
/// `source` 是这份文本来自哪个文件，只用于显示（引导页的「来自 …」前缀）。
/// 规则全在那唯一一份解析器里：`Host` / `HostName` / `User` / `Port` /
/// `IdentityFile` / `ProxyJump`，关键字大小写不敏感，`Key Value` 与
/// `Key = Value` 都认，`#` / `;` 注释（含行尾注释）会被裁掉，`Host *` 与
/// `Match` 段不生成主机，`Host a b c` 一行多个别名展开成三条。
/// 行为由 `onboarding_tests.rs` 的 10 条金样本钉住。
fn parse_ssh_config(text: &str, source: &str) -> Vec<DetectedHost> {
    adapt(parse_ssh(text), source)
}

/// 解析 WindTerm `profiles/` 里一份导出的文本（JSON 或 INI，两种都认）。
///
/// `docs/design/09-session-import-and-onboarding.md` 的另一半：WindTerm 自己的会话也要能预填进来，
/// 走同一个适配器，所以 `source` 与 `label` 的语义与 ssh 那条完全一致。
fn parse_windterm_profiles(text: &str, source: &str) -> Vec<DetectedHost> {
    adapt(parse_windterm(text), source)
}

/// 探测本机已有的 SSH 主机：`~/.ssh/config` + WindTerm 的 `profiles/`。
///
/// 读文件、解析、适配都在这一层；读不到就是探不到——不伪造、也不因为它们
/// 不存在而报错。第一台机器上什么都读不到是**正常**的。
fn detect_hosts() -> Vec<DetectedHost> {
    let mut out = Vec::new();
    let Some(home) = dirs::home_dir() else {
        return out;
    };

    let ssh_config = home.join(".ssh").join("config");
    if let Ok(text) = std::fs::read_to_string(&ssh_config) {
        out.extend(parse_ssh_config(&text, &ssh_config.to_string_lossy()));
    }

    // 与侧栏共用发现层的唯一入口：候选根 + 一层子目录里的 `.wind` / `.json`
    // 导出。此前引导自己写 `read_dir(root)` 只看一层，侧栏加了子目录支持后
    // 两边口径就分叉了。发现层只枚举有限目录，不扫描全盘。
    let roots = term_import::discovery::candidate_windterm_roots();
    let exports = term_import::discovery::find_profile_exports_in(&roots);
    for path in exports.profiles {
        if let Ok(text) = std::fs::read_to_string(&path) {
            out.extend(parse_windterm_profiles(&text, &path.to_string_lossy()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_source_strips_the_home_prefix_when_there_is_one() {
        // 家目录在哪台机器上都不同，所以这条只在「有家目录」时断言前缀规则，
        // 没有家目录（CI 的某些容器）时退化成「原样返回」也不算失败。
        if let Some(home) = dirs::home_dir() {
            let home = home.to_string_lossy().into_owned();
            let got = short_source(&format!("{home}/.ssh/config"));
            assert_eq!(got, "~/.ssh/config");
        }
        assert_eq!(short_source("/etc/ssh/ssh_config"), "/etc/ssh/ssh_config");
    }

    #[test]
    fn the_import_group_is_not_a_translated_chrome_string() {
        // 分组名是数据（`term_app::i18n` 模块文档），所以它不该在词表里被翻译成
        // 别的样子——这条钉住「它保持英文原文」这件事，免得有人顺手把它塞进 zh()。
        assert_eq!(IMPORT_GROUP, "Imported");
        assert_eq!(term_app::i18n::t(Lang::Zh, "Imported"), "Imported");
    }

    fn detected(label: &str, host: &str) -> DetectedHost {
        DetectedHost {
            label: label.to_string(),
            host: host.to_string(),
            user: "deploy".to_string(),
            port: 2222,
            source: "~/.ssh/config".to_string(),
        }
    }

    #[test]
    fn merge_appends_sessions_and_creates_the_group_once() {
        let mut sessions: Vec<SessionConfig> = vec![SessionConfig::new(Protocol::Shell)];
        let mut groups: Vec<String> = vec!["Servers".to_string()];
        let added = merge_detected(
            &mut sessions,
            &mut groups,
            vec![detected("web", "10.0.0.11"), detected("db", "db.example")],
        );
        assert_eq!(added, 2);
        assert_eq!(sessions.len(), 3, "原有的本地 shell 不许被顶掉");
        // 名字、目标、分组、协议四件事逐条钉住。
        let last = &sessions[2];
        assert_eq!(last.name, "db");
        assert_eq!(last.host, "db.example");
        assert_eq!(last.port, 2222);
        assert_eq!(last.username, "deploy");
        assert_eq!(last.group, IMPORT_GROUP);
        assert_eq!(last.protocol, Protocol::Ssh);
        assert!(last.builtin_ssh);
        assert_eq!(
            groups.iter().filter(|g| *g == IMPORT_GROUP).count(),
            1,
            "分组只建一次"
        );
        // 再导一次：分组也不该出现第二份。
        merge_detected(&mut sessions, &mut groups, vec![detected("api", "a.example")]);
        assert_eq!(groups.iter().filter(|g| *g == IMPORT_GROUP).count(), 1);
    }

    #[test]
    fn merge_renames_a_collision_instead_of_dropping_or_overwriting() {
        // 会话树里已经有一条叫 `web` 的：既有实现（侧栏导入）是**跳过**，
        // 引导这里选**改名**——用户刚在清单上勾了这台机器，静默丢掉一条会
        // 让人以为按钮没生效。
        let mut existing = SessionConfig::new(Protocol::Ssh);
        existing.name = "web".to_string();
        let mut sessions: Vec<SessionConfig> = vec![existing.clone()];
        let mut groups: Vec<String> = Vec::new();
        merge_detected(&mut sessions, &mut groups, vec![detected("web", "10.0.0.11")]);
        assert_eq!(sessions.len(), 2, "一条都不许丢");
        assert_eq!(
            sessions[0].name, "web",
            "原来那条会话的字段一个字都不许动"
        );
        assert_eq!(sessions[0].host, existing.host);
        assert!(
            sessions[1].name.starts_with("web (copy"),
            "撞名走既有的 unique_name 后缀：{}",
            sessions[1].name
        );
        assert_eq!(sessions[1].host, "10.0.0.11", "新来那条带着自己的目标");
    }

    #[test]
    fn windterm_user_sessions_keep_their_protocol_when_merged() {
        // DetectedHost 把协议硬编码成 Ssh，所以 user.sessions 不能摊成它；
        // 走 ImportedSession 落库时 Telnet 必须还是 Telnet。
        let items = parse_windterm_sessions(
            r#"[{"session.protocol":"Telnet","session.label":"switch",
                 "session.target":"admin@10.0.0.9:2323","session.group":"Net"}]"#,
        );
        assert_eq!(items.len(), 1);
        let mut sessions = Vec::new();
        let mut groups = Vec::new();
        assert_eq!(merge_imported(&mut sessions, &mut groups, items), 1);
        assert_eq!(sessions[0].protocol, Protocol::Telnet);
        assert_eq!(sessions[0].host, "10.0.0.9");
        assert_eq!(sessions[0].port, 2323);
        assert_eq!(sessions[0].username, "admin");
        assert!(groups.iter().any(|g| g == "Net"));
    }

    #[test]
    fn merge_with_nothing_adds_no_session_and_no_group() {
        let mut sessions: Vec<SessionConfig> = Vec::new();
        let mut groups: Vec<String> = Vec::new();
        let added = merge_detected(&mut sessions, &mut groups, Vec::new());
        assert_eq!(added, 0);
        assert!(sessions.is_empty());
        assert!(groups.is_empty(), "空导入不该在树上留一个空文件夹");
    }
}

// 解析适配的 11 条单测随包装函数一起从 `term-app::onboarding` 搬到这里。
// 单独成文件而不是塞进上面的 `mod tests`：按「非空非注释行」数 ui/ 单文件，
// 本文件已经装了读文件 / 绘制 / 落地三段。挂载方式与 `preview_tests.rs` 同一套
// （`#[cfg(test)]` + `#[path = …]` 两行声明，会把整份文件当纯测试模块跳过）。
#[cfg(test)]
#[path = "onboarding_tests.rs"]
mod parse_tests;
