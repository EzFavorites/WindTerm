//! 会话日志的**选项**（配置面）与两个模板展开函数（文件名 / 内容行）。
//!
//! 为什么在 `term-core`：写入端在 `term-io`、配置界面在 `apps/desktop`、
//! 偏好文件在 `term-profile`，**三处都要用同一份语义**。放任何一处都会让
//! 另外两处要么复制一份、要么反向依赖。这里是纯函数：零 IO、零时钟、
//! 零线程——`secs` 一律由调用方传进来。
//!
//! # 三个字段各管什么（按视觉基线截图定，2026-11-14）
//!
//! `Session Settings ▸ Logging` 视觉基线里，`Log nothing` 选中时 `Start...`
//! 菜单项**仍然是亮的**。所以 `Log types` 管的是「会话建立时要不要自动记」，
//! 不是「手动能不能记」——手动点了 Start 就是明确要记。这条口径落成
//! [`LogType::manual_mode`]：`Nothing` 手动记时按 `Printable` 走。
//!
//! 另外两个字段是纯模板：
//! - `Log file name`：`%n` = 会话名（折成文件名安全字符），另有 `%Y %y %m %d
//!   %H %M %S` 与 `%%`。出厂值 `%n_%Y-%m-%d_%H-%M-%S.log`。
//! - `Log content pattern`：`%v` = 输出的一行，其余同上。出厂值
//!   `[%Y-%m-%d %H:%M:%S] %v`。**空模板 = 不加前缀**（老 `prefs.json` 的
//!   空串是「没设置过」，由 [`LogOptions::normalized`] 回落出厂值，不是「不加前缀」）。

use std::fmt::Write as _;

/// 文件名模板的出厂值（逐字取自视觉基线）。
pub const DEFAULT_FILE_NAME: &str = "%n_%Y-%m-%d_%H-%M-%S.log";

/// 内容模板的出厂值（逐字取自视觉基线）。
pub const DEFAULT_CONTENT: &str = "[%Y-%m-%d %H:%M:%S] %v";

/// 文件名展开后的上限。`NAME_MAX` 在多数文件系统是 255 字节，留一半够用，
/// 也避免一个手滑的长模板让 `create` 报 `ENAMETOOLONG`。
const NAME_MAX_CHARS: usize = 128;

/// 记什么 / 记不记。序号落 `prefs.json`（`log_type` 字段），所以顺序是契约。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogType {
    /// 出厂态：会话建立时**不**自动记。
    #[default]
    Nothing,
    /// 清洗后的可打印文本（去掉颜色与光标）。
    Printable,
    /// 原始字节，含转义序列。能完整回放，但 `grep` 不到干净的一行。
    All,
}

impl LogType {
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Nothing => 0,
            Self::Printable => 1,
            Self::All => 2,
        }
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Printable,
            2 => Self::All,
            _ => Self::Nothing,
        }
    }

    /// 只有真的要记点什么，覆盖 / 追加才有意义（视觉基线出厂态两行全灰）。
    pub fn enabled(self) -> bool {
        self != Self::Nothing
    }

    /// 手动 Start 实际使用的模式。
    ///
    /// `Nothing` → `Printable`：见文件头那段口径（视觉基线里 Start 是亮的）。
    pub fn manual_mode(self) -> Self {
        match self {
            Self::Nothing => Self::Printable,
            other => other,
        }
    }
}

/// 一次记录的全部选项。字段名与 `Session Settings ▸ Logging` 一一对应。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogOptions {
    pub log_type: LogType,
    /// `None` = 视觉基线出厂态（既不覆盖也不追加，靠序号避让）。
    /// `Some(true)` = 覆盖同名文件；`Some(false)` = 追加到同名文件。
    pub overwrite: Option<bool>,
    pub file_name: String,
    /// 0 = ∞。
    pub max_size_mb: u32,
    pub content_pattern: String,
}

impl LogOptions {
    /// 空串回落出厂值。老 `prefs.json` 没有这两项，读出来就是空串——那表示
    /// 「没设置过」，不是「用户把模板改成了空」。
    pub fn normalized(mut self) -> Self {
        if self.file_name.trim().is_empty() {
            self.file_name = DEFAULT_FILE_NAME.to_string();
        }
        if self.content_pattern.trim().is_empty() {
            self.content_pattern = DEFAULT_CONTENT.to_string();
        }
        self
    }

    /// 手动 Start 用的模式（`Nothing` 落 `Printable`）。
    pub fn mode(&self) -> LogType {
        self.log_type.manual_mode()
    }

    /// 轮转阈值（字节）。`None` = ∞。
    pub fn max_bytes(&self) -> Option<u64> {
        (self.max_size_mb > 0).then(|| self.max_size_mb as u64 * 1024 * 1024)
    }

    /// 文件名（已做安全折叠）。永远非空。
    pub fn file_name_for(&self, session: &str, secs: u64) -> String {
        self.file_name_at(session, None, LogStamp::from_secs(secs))
    }

    /// 文件名（带宿主名与毫秒，ADR-0027 §S5）。
    ///
    /// `host` 只喂 `%{host}`——不传就是空串（**不**回落到会话名：那会让
    /// 「文件名里有没有主机名」变成运气）。
    pub fn file_name_env(
        &self,
        session: &str,
        host: Option<&str>,
        stamp: LogStamp,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> String {
        let name = self.clone().normalized().file_name;
        let raw = expand_template(&name, session, host, None, stamp, env);
        finish_name(&raw, session)
    }

    pub fn file_name_at(&self, session: &str, host: Option<&str>, stamp: LogStamp) -> String {
        // `%{env:NAME}` 的取值**必须由调用方给**：本模块是纯语义层，读环境变量
        // 属于副作用（`scripts/verify.sh` 的 term-core 判据盯着这一条）。写成
        // 「这里返回 None」而不是「这里读 env」——真读的地方是 `term-io` 的
        // 日志写入端，它把 `std::env::var` 作为闭包传进来。
        self.file_name_env(session, host, stamp, &|_| None)
    }

    /// 内容模板是否是「原样输出」（`%v` 或空）。原样时不做逐行切分，省一次分配。
    pub fn content_is_plain(&self) -> bool {
        let t = self.content_pattern.trim();
        t.is_empty() || t == "%v"
    }

    /// 一行输出套上内容模板。
    pub fn line(&self, session: &str, body: &str, secs: u64) -> String {
        expand(&self.content_pattern, session, Some(body), secs)
    }

    /// 一行输出（带宿主名与毫秒；`%{env:NAME}` 由调用方提供取值）。
    pub fn line_env(
        &self,
        session: &str,
        host: Option<&str>,
        body: &str,
        stamp: LogStamp,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> String {
        expand_template(&self.content_pattern, session, host, Some(body), stamp, env)
    }

    /// 一行输出（带宿主名与毫秒）。
    pub fn line_at(&self, session: &str, host: Option<&str>, body: &str, stamp: LogStamp) -> String {
        // 同 `file_name_at`：env 由调用方给（`term-io` 的写入端注入），
        // 本模块不读环境。要 `%{env:}` 请走 `line_env`。
        self.line_env(session, host, body, stamp, &|_| None)
    }
}

/// 日志时间戳：秒（文件名模板的老口径）+ 毫秒（`%{ms}`）。
///
/// 为什么不让 `expand` 自己去取钟：`term-core` 零墙钟（I3），模板展开必须是
/// 纯函数——同一份模板 + 同一个 stamp 展开出同一个名字，文件名才可复现。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LogStamp {
    pub secs: u64,
    pub millis: u32,
}

impl LogStamp {
    /// 只有秒精度（毫秒按 0 算）。
    pub fn from_secs(secs: u64) -> Self {
        Self { secs, millis: 0 }
    }
}

/// 模板展开（ADR-0027 §S5 的兼容扩展）。
///
/// 既有语义**一个都不改**（改了会破坏用户已存的模板）：`%Y %y %m %d %H %M %S %n %v %%`。
/// 新增的是花括号形态，与 CRT 的单字母宏不冲突：
///
/// | 扩展 | 含义 |
/// |---|---|
/// | `%{host}` | 主机名（拿不到时为空串） |
/// | `%{session}` | 会话名（已 slug 化，可直接进文件名） |
/// | `%{ms}` | 毫秒部分（三位补零） |
/// | `%{env:NAME}` | 环境变量；没有这个变量时展开为空串 |
///
/// 注意 CRT 的 `%H` 是**主机名**、`%S` 是**会话名**，与我们的 `%H`=小时、
/// `%S`=秒正好相反——所以这里不重定义单字母宏，只提供花括号形态，界面上
/// 再给一个「把 CRT 模板转换过来」的按钮（`crt_template_to_ours`）。
pub fn expand_template(
    tpl: &str,
    session: &str,
    host: Option<&str>,
    body: Option<&str>,
    stamp: LogStamp,
    env: &dyn Fn(&str) -> Option<String>,
) -> String {
    let mut out = String::with_capacity(tpl.len() + 32);
    let mut it = tpl.chars().peekable();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        if it.peek() == Some(&'{') {
            it.next();
            let mut name = String::new();
            let mut closed = false;
            for ch in it.by_ref() {
                if ch == '}' {
                    closed = true;
                    break;
                }
                name.push(ch);
            }
            if closed {
                match name.as_str() {
                    "host" => out.push_str(host.unwrap_or("")),
                    "session" => out.push_str(&slug(session)),
                    "ms" => {
                        let _ = write!(out, "{:03}", stamp.millis % 1000);
                    }
                    other => {
                        if let Some(var) = other.strip_prefix("env:") {
                            if let Some(value) = env(var) {
                                out.push_str(&value);
                            }
                        } else {
                            // 未知扩展原样保留：打错了要看得见，不能悄悄变空。
                            out.push_str("%{");
                            out.push_str(other);
                            out.push('}');
                        }
                    }
                }
            } else {
                // 没有闭合：原样保留（同上）。
                out.push_str("%{");
                out.push_str(&name);
            }
            continue;
        }
        // 单字母宏：沿用既有实现，避免两套语义。
        out.push_str(&expand_one(it.next(), session, body, stamp.secs));
    }
    out
}

/// CRT 文件名模板 → 本仓模板（`%H` 主机 / `%S` 会话 / `%M` 月 / `%m` 分 / `%h` 小时 / `%t` 毫秒）。
///
/// 只做**映射**，不做猜测：CRT 模板里我们认识的宏换成花括号扩展，不认识的
/// 原样保留（用户能看见哪几个没转）。
pub fn crt_template_to_ours(tpl: &str) -> String {
    let mut out = String::with_capacity(tpl.len() + 16);
    let mut it = tpl.chars();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('H') => out.push_str("%{host}"),
            Some('S') => out.push_str("%{session}"),
            Some('M') => out.push_str("%m"),
            Some('m') => out.push_str("%M"),
            Some('h') => out.push_str("%H"),
            Some('t') => out.push_str("%{ms}"),
            Some('%') => out.push_str("%%"),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// 单个 `%X` 的展开（文件名与内容模板共用）。
fn expand_one(next: Option<char>, session: &str, body: Option<&str>, secs: u64) -> String {
    let (y, mo, d, h, mi, s) = ymd_hms(secs);
    let mut out = String::new();
    match next {
        Some('Y') => {
            let _ = write!(out, "{y:04}");
        }
        Some('y') => {
            let _ = write!(out, "{:02}", y.rem_euclid(100));
        }
        Some('m') => {
            let _ = write!(out, "{mo:02}");
        }
        Some('d') => {
            let _ = write!(out, "{d:02}");
        }
        Some('H') => {
            let _ = write!(out, "{h:02}");
        }
        Some('M') => {
            let _ = write!(out, "{mi:02}");
        }
        Some('S') => {
            let _ = write!(out, "{s:02}");
        }
        Some('n') => out.push_str(&slug(session)),
        Some('v') => match body {
            Some(b) => out.push_str(b),
            // 不认识的上下文里保留原样：静默吃掉比留着难查。
            None => out.push_str("%v"),
        },
        Some('%') => out.push('%'),
        // 未知占位符原样保留：用户打错了要看得见，不能悄悄变成空。
        Some(other) => {
            out.push('%');
            out.push(other);
        }
        None => out.push('%'),
    }
    out
}

/// 文件名模板展开。`body` 为 `None` 时 `%v` 原样留着（文件名里没人用它）。
fn expand(tpl: &str, session: &str, body: Option<&str>, secs: u64) -> String {
    expand_template(tpl, session, None, body, LogStamp::from_secs(secs), &|_| None)
}

/// 文件名安全化：目录分隔符与 NUL 折成 `-`，控制字符丢掉，去首尾空白，
/// 限长；`.` / `..` 这类整体名字直接作废（返回空，由调用方回落）。
///
/// 模板是用户写的，允许它含 `/` 就等于允许日志写到配置目录外面去。
/// 折叠成安全文件名；折叠后为空（模板里全是非法字符）时退回会话名。
///
/// 抽出来是因为 `file_name_at` 与 `file_name_env` 只差「env 从哪来」，
/// 收尾逻辑写两遍就会漂（一条改了点号规则另一条没改）。
fn finish_name(raw: &str, session: &str) -> String {
    let safe = safe_name(raw);
    if safe.is_empty() {
        format!("{}.log", slug(session))
    } else {
        safe
    }
}

fn safe_name(raw: &str) -> String {
    let folded: String = raw
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .take(NAME_MAX_CHARS)
        .collect();
    let trimmed = folded.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return String::new();
    }
    trimmed.to_string()
}

/// 会话名 → 文件名里的一段。
///
/// 只留 ASCII 字母数字与 `.` `_`，其余（含中文、斜杠、空格）折成 `-`。
/// 中文会话名会全部折掉，于是落到 `session`——用时间戳区分；会话名本身记在
/// 文件头里。之所以不保留 Unicode：日志文件名会进 `grep`、进脚本、进别人的
/// `ls` 输出，一个既有空格又有斜杠的名字（`prod/db 备份`）在这三处都要转义。
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' {
            out.push(ch);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let trimmed: String = out.trim_matches('-').chars().take(48).collect();
    if trimmed.is_empty() {
        "session".to_string()
    } else {
        trimmed
    }
}

/// Unix 秒 → 年月日时分秒（UTC）。
///
/// 手写而不是拉一个日期库进来：只需要「epoch 加一个整数」，为此多一个依赖
/// 不值得；而且 UTC 明确，不涉及本地时区与夏令时那套真正复杂的东西。
pub fn ymd_hms(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    (
        y,
        m,
        d,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

/// `2026-11-05T08:24:12Z`：给人看的时间戳（文件头 / 暂停标记用）。
pub fn iso(secs: u64) -> String {
    let (y, mo, d, h, mi, s) = ymd_hms(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Howard Hinnant 的 `civil_from_days`：把「1970-01-01 起的日数」换成公历。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-05T08:24:12Z（与 `log.rs` 的既有判据同一个锚点，别自己算）。
    const T: u64 = 1_793_867_052;

    #[test]
    fn the_default_name_template_matches_the_reference_screenshot() {
        let opts = LogOptions::default().normalized();
        assert_eq!(
            opts.file_name_for("prod-db", T),
            "prod-db_2026-11-05_08-24-12.log"
        );
    }

    #[test]
    fn a_session_name_that_is_not_filename_safe_is_folded_but_never_escapes_the_dir() {
        let opts = LogOptions::default();
        // 中文整段折掉 → `session` 兜底（与 R14-a 的取值一致）。
        assert_eq!(opts.file_name_for("生产库", T), "session_2026-11-05_08-24-12.log");
        // 斜杠不能变成路径分隔符。
        let name = opts.file_name_for("prod/db 备份", T);
        assert!(!name.contains('/'), "{name}");
        assert_eq!(name, "prod-db_2026-11-05_08-24-12.log");
    }

    #[test]
    fn a_hand_written_template_that_tries_to_walk_out_of_the_dir_is_defused() {
        // 用户可以在设置页里写任意模板：`%n` 之外的字面量也必须过一遍折叠。
        let opts = LogOptions {
            file_name: "../../etc/passwd".to_string(),
            ..Default::default()
        };
        let name = opts.file_name_for("x", T);
        // 真正的安全属性是「仍然只是一个文件名」——一个路径分隔符都不能剩。
        assert_eq!(
            std::path::Path::new(&name).components().count(),
            1,
            "{name} 还能走出目录"
        );
        assert!(!name.contains('/'), "{name}");
        // 整个模板就是 `..`/`.` 时作废，回落会话名（否则会指向父目录）。
        for hostile in ["..", "."] {
            let o = LogOptions {
                file_name: hostile.to_string(),
                ..Default::default()
            };
            assert_eq!(o.file_name_for("web", T), "web.log", "{hostile}");
        }
        // 纯空白不是「恶意模板」，是「没设置过」——回落到出厂模板。
        let blank = LogOptions {
            file_name: "   ".to_string(),
            ..Default::default()
        };
        assert_eq!(blank.file_name_for("web", T), "web_2026-11-05_08-24-12.log");
    }

    #[test]
    fn an_unknown_placeholder_stays_visible_instead_of_disappearing() {
        // 打错 `%q` 要看得见——静默吃掉的模板最难查。
        let opts = LogOptions {
            file_name: "%n_%q.log".to_string(),
            ..Default::default()
        };
        assert_eq!(opts.file_name_for("web", T), "web_%q.log");
        let content = LogOptions {
            content_pattern: "%v %q".to_string(),
            ..Default::default()
        };
        assert_eq!(content.line("web", "hi", T), "hi %q");
        // 模板末尾孤零零一个 `%` 也不能吞掉。
        let trailing = LogOptions {
            content_pattern: "%v%".to_string(),
            ..Default::default()
        };
        assert_eq!(trailing.line("web", "hi", T), "hi%");
    }

    #[test]
    fn the_content_pattern_wraps_each_line_with_its_own_timestamp() {
        let opts = LogOptions::default().normalized();
        assert_eq!(
            opts.line("web", "ls -l", T),
            "[2026-11-05 08:24:12] ls -l"
        );
        // `%v` 出现两次就出现两次（不是特殊照顾的一次性替换）。
        let twice = LogOptions {
            content_pattern: "%v|%v".to_string(),
            ..Default::default()
        };
        assert_eq!(twice.line("web", "a", T), "a|a");
    }

    #[test]
    fn an_empty_pref_means_never_set_not_deliberately_blank() {
        // 老 prefs.json 读出来是空串：要回落到视觉基线出厂值，而不是「不加前缀」。
        let old = LogOptions {
            file_name: String::new(),
            content_pattern: "   ".to_string(),
            ..Default::default()
        }
        .normalized();
        assert_eq!(old.file_name, DEFAULT_FILE_NAME);
        assert_eq!(old.content_pattern, DEFAULT_CONTENT);
        // 但显式的 `%v` 就是「原样输出」，不套前缀。
        let plain = LogOptions {
            content_pattern: "%v".to_string(),
            ..Default::default()
        };
        assert!(plain.content_is_plain());
    }

    #[test]
    fn log_nothing_still_logs_when_the_user_asks_by_hand() {
        // 视觉基线：`Log nothing` 选中时 `Start...` 仍是亮的 —— 这个字段管的是
        // 「会话建立时自动不自动记」，不是「手动能不能记」。
        assert_eq!(LogType::Nothing.manual_mode(), LogType::Printable);
        assert_eq!(LogType::All.manual_mode(), LogType::All);
        assert!(!LogType::Nothing.enabled());
        assert!(LogType::Printable.enabled());
        // 序号是 prefs.json 的契约，钉住它防止有人重排枚举。
        for v in [LogType::Nothing, LogType::Printable, LogType::All] {
            assert_eq!(LogType::from_u8(v.as_u8()), v);
        }
    }

    #[test]
    fn the_size_cap_is_none_only_when_it_is_zero() {
        let inf = LogOptions::default();
        assert_eq!(inf.max_bytes(), None, "0 = ∞（视觉基线那一格显示的就是 ∞）");
        let capped = LogOptions {
            max_size_mb: 8,
            ..Default::default()
        };
        assert_eq!(capped.max_bytes(), Some(8 * 1024 * 1024));
    }

    #[test]
    fn brace_expansions_do_not_disturb_the_single_letter_macros() {
        let stamp = LogStamp { secs: 0, millis: 42 };
        let text = expand_template(
            "%Y-%m-%d %H:%M:%S %n %{host}:%{session}.%{ms}",
            "prod/db",
            Some("db1.example"),
            None,
            stamp,
            &|_| None,
        );
        // 老宏照旧（`%H` 仍是小时，不是主机名——语义不改）。
        assert!(text.starts_with("1970-01-01 00:00:00"), "{text}");
        assert!(text.contains("prod-db"), "{text}");
        assert!(text.contains("db1.example:prod-db.042"), "{text}");
    }

    #[test]
    fn a_missing_host_is_empty_not_the_session_name() {
        let stamp = LogStamp::from_secs(0);
        let text = expand_template("%{host}|%{session}", "web", None, None, stamp, &|_| None);
        assert_eq!(text, "|web");
    }

    #[test]
    fn env_expansion_uses_the_supplied_lookup_and_leaves_unknown_macros_alone() {
        let stamp = LogStamp::from_secs(0);
        let env = |name: &str| (name == "HOME").then(|| "/home/qa".to_string());
        let text = expand_template("%{env:HOME}/%{env:NOPE}/%{bogus}", "s", None, None, stamp, &env);
        assert_eq!(text, "/home/qa//%{bogus}", "未知扩展原样保留（打错要看得见）");
    }

    #[test]
    fn crt_templates_are_translated_not_reinterpreted() {
        // CRT：%H 主机 / %S 会话 / %M 月 / %m 分 / %h 小时 / %t 毫秒
        // 本仓：%H 小时 / %S 秒 / %M 分 / %m 月
        assert_eq!(
            crt_template_to_ours("%H-%S_%M%m%h%t.log"),
            "%{host}-%{session}_%m%M%H%{ms}.log"
        );
        // 不认识的宏原样留着。
        assert_eq!(crt_template_to_ours("%Q"), "%Q");
    }
}
