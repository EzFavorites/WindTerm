//! 脚本录制器（ADR-0027）：把**用户敲进去的字节**和**屏幕静默点**录成 `.rhai`。
//!
//! # 为什么录「敲了什么」而不是「调了什么 API」
//!
//! SecureCRT 的录制器录的是人的动作：一串击键 + 屏幕什么时候安静下来。这样生成
//! 的脚本与手写脚本**长得一样**（`send` 一段、等一个提示符、再 `send` 一段），
//! 用户看得懂也改得动。反过来，如果把录制做成「记录脚本调用的原语序列」，录出来
//! 的东西会把 `session_use` / `wait_idle` 这类实现细节全带进来，人读不了，也
//! 没法拿它当「我刚才干了什么」的说明书。
//!
//! # 三个纪律
//!
//! 1. **只录人的输入**。设备应答（DSR / DA 的回报）、粘贴走的自动换行、界面自己
//!    补的字节都不是人敲的，录进去会让回放多出一堆莫名其妙的字符。调用点在键盘
//!    漏斗那一处（`ui/terminal/pane.rs`），设备应答从不到那里。
//! 2. **静默点才落一步**。屏幕还在动的时候落 `read_until` 等于猜一个还没出现的
//!    字符串；等它安静下来，最后一行就是最稳的锚点。
//! 3. **生成物必须能直接跑**。转义由 [`rhai_literal`] 一处负责，测试拿真的引擎
//!    跑一遍生成物（`a_recorded_session_can_be_replayed`），而不是只比字符串。

use std::time::{Duration, Instant};

/// 多久没动静算「屏幕安静了」。
///
/// 400ms 是折中：SSH 一条命令的回显通常几十毫秒，但慢网络下最后一段可能拖到
/// 两三百毫秒；再长会让录制的人觉得「我停了它还在等」，再短就会把一条命令的
/// 回显拆成两步。
pub const QUIET: Duration = Duration::from_millis(400);

/// 录下来的一步。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// 往会话里写这些字节（人敲的原文，含回车）。
    Send(String),
    /// 等到屏幕上出现这段文本。
    Wait(String),
}

/// 一次录制。
pub struct Recorder {
    steps: Vec<Step>,
    /// 攒着还没落成 `Send` 的击键。
    pending: String,
    /// 屏幕上最后一段非空文本（下一处 `Wait` 的锚点）。
    tail: String,
    last_activity: Instant,
    /// 当前这一段已经落过 `Wait` 了（避免同一段静默反复落）。
    settled: bool,
}

impl Recorder {
    pub fn new(now: Instant) -> Self {
        Self {
            steps: Vec::new(),
            pending: String::new(),
            tail: String::new(),
            last_activity: now,
            settled: true,
        }
    }

    /// 人敲进去的字节。原样收下：回车就是 `\r`，方向键就是 `\x1b[A`。
    pub fn note_input(&mut self, text: &str, now: Instant) {
        if text.is_empty() {
            return;
        }
        self.pending.push_str(text);
        self.last_activity = now;
        self.settled = false;
    }

    /// 屏幕上最后一段非空文本变了。
    ///
    /// 只在**真的变了**的时候刷新时间戳：每帧都刷新的话「安静」永远不会到来。
    pub fn note_screen(&mut self, tail: &str, now: Instant) {
        if tail == self.tail {
            return;
        }
        self.tail = tail.to_string();
        self.last_activity = now;
        self.settled = false;
    }

    /// 每帧调一次：安静够久就把这一段落成 `Send` + `Wait`。
    pub fn tick(&mut self, now: Instant) {
        if self.settled || self.pending.is_empty() {
            return;
        }
        if now.saturating_duration_since(self.last_activity) < QUIET {
            return;
        }
        self.flush(now);
        // 落完之后把锚点补上：屏幕停在最后那一行，下一条命令发出去之前它还在。
        if !self.tail.is_empty() {
            self.steps.push(Step::Wait(self.tail.clone()));
        }
        self.settled = true;
    }

    /// 结束录制，把还没落的那一段刷出来（最后一条命令通常等不到静默点）。
    ///
    /// 返回录到的步。空串 / 全空的击键不会留下步骤。
    pub fn finish(&mut self, now: Instant) -> Vec<Step> {
        self.flush(now);
        self.settled = true;
        std::mem::take(&mut self.steps)
    }

    /// 已经录到的步（不含还没落的那一段）。状态栏显示条数用。
    pub fn len(&self) -> usize {
        self.steps.len() + usize::from(!self.pending.is_empty())
    }

    /// 还什么都没录到（只按下录制按钮就停 = 不生成文件）。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn flush(&mut self, now: Instant) {
        if self.pending.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.pending);
        self.steps.push(Step::Send(text));
        self.last_activity = now;
    }
}

/// 把一段文本写成 Rhai 字符串字面量。
///
/// 只有这一处负责转义：录进去的是原始字节（可能带 `\r`、ESC、引号），生成物要
/// 能被引擎原样读回来。控制字符用 `\u00XX` 而不是直接塞字节——塞进去的字节在
/// 源码里是非法 UTF-8 之外的不可见字符，用户打开文件会看到一堆问号。
pub fn rhai_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 每步 `read_until` 的默认超时（秒）。
///
/// 10 秒与手工脚本里的常见写法一致：够慢网络回一条命令，又不至于让一条没等到
/// 的命令把整个脚本卡到沙箱墙钟上。
pub const WAIT_SECS: f64 = 10.0;

/// 把步序列写成一份可运行的脚本。
///
/// 文件头写明来历：一份来路不明的 `.rhai` 混进脚本库之后，用户第一眼该知道
/// 它不是手写的（录制物通常要改，比如把写死的路径换成 `args(0)`）。
pub fn to_script(steps: &[Step]) -> String {
    let mut out = String::from(
        "// Recorded by zterm (Script \u{25b8} Record). Adjust paths and prompts before reusing.\n",
    );
    for step in steps {
        match step {
            Step::Send(text) => {
                out.push_str("write(");
                out.push_str(&rhai_literal(text));
                out.push_str(");\n");
            }
            Step::Wait(text) => {
                out.push_str("read_until(");
                out.push_str(&rhai_literal(text));
                out.push_str(&format!(", {WAIT_SECS});\n"));
            }
        }
    }
    out
}

/// 把生成物写进脚本库，返回落地的路径。
///
/// 名字按 `recorded.rhai` / `recorded-2.rhai` 往后找空位：与「新建脚本」同一个
/// 理由——每次录制都给同一个文件名，第二次会把第一次录的静默盖掉。
pub fn save_to_library(text: &str) -> Result<std::path::PathBuf, String> {
    let dir = term_runtime::paths::scripts_dir()
        .ok_or_else(|| "找不到脚本库目录（没有家目录？）".to_string())?;
    // 边界仍只有这一处 `String`；内部用 `io::Error`，免得给 4q 的错误边界
    // 基线再添一条。文案在这里一次翻好。
    save_to_library_in(&dir, text).map_err(|e| e.to_string())
}

/// 真正的落库逻辑；目录由调用方给，便于单测用临时目录而不碰进程级环境变量。
fn save_to_library_in(
    dir: &std::path::Path,
    text: &str,
) -> Result<std::path::PathBuf, std::io::Error> {
    std::fs::create_dir_all(dir)?;
    for index in 1..1000 {
        let name = if index == 1 {
            "recorded.rhai".to_string()
        } else {
            format!("recorded-{index}.rhai")
        };
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        std::fs::write(&path, text)?;
        return Ok(path);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("脚本库里已经有太多 recorded-*.rhai（{}）", dir.display()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> Instant {
        // 固定起点 + 偏移：不读墙钟，用例之间互不影响。
        Instant::now() + Duration::from_millis(ms)
    }

    #[test]
    fn three_typed_commands_become_three_sends_and_their_quiet_points() {
        // 三条命令：每条敲完、屏幕安静一次，最后再收尾。
        let mut rec = Recorder::new(at(0));
        rec.note_input("ls\r", at(0));
        rec.note_screen("$ ", at(20));
        rec.tick(at(600));
        rec.note_input("pwd\r", at(700));
        rec.note_screen("/home$ ", at(720));
        rec.tick(at(1300));
        rec.note_input("exit\r", at(1400));
        let steps = rec.finish(at(1500));

        assert_eq!(
            steps,
            vec![
                Step::Send("ls\r".to_string()),
                Step::Wait("$ ".to_string()),
                Step::Send("pwd\r".to_string()),
                Step::Wait("/home$ ".to_string()),
                Step::Send("exit\r".to_string()),
            ]
        );
    }

    #[test]
    fn a_screen_that_is_still_moving_does_not_close_a_step() {
        // 屏幕还在动就落 `read_until` 等于猜一个还没出现的字符串。
        let mut rec = Recorder::new(at(0));
        rec.note_input("make\r", at(0));
        rec.note_screen("compiling 1/9", at(100));
        rec.tick(at(300));
        rec.note_screen("compiling 2/9", at(350));
        rec.tick(at(500));
        assert!(rec.steps.is_empty(), "还在刷屏时不该落步");
        assert_eq!(rec.len(), 1, "但那一行输入已经攒着了");
    }

    #[test]
    fn pressing_record_and_stopping_right_away_records_nothing() {
        let mut rec = Recorder::new(at(0));
        rec.note_screen("$ ", at(10));
        assert!(rec.is_empty());
        assert_eq!(rec.finish(at(20)), Vec::new());
        assert!(to_script(&[]).contains("Recorded by zterm"), "头还是要有，说明它为什么是空的");
    }

    #[test]
    fn a_recorded_session_can_be_replayed() {
        // 判据④的机器版：录一段「敲一条命令等回显」，把生成物丢回引擎里跑通。
        // 只比字符串的话，转义写错、`read_until` 的参数顺序写反都查不出来。
        use crate::script_run::{Outcome, RunOptions, ScriptRun};

        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let id = pool.open_exec("cat", 80, 24).expect("起不了 `cat`（本机 PTY 不可用？）");

        let mut rec = Recorder::new(at(0));
        rec.note_input("hi\r", at(0));
        // `cat` 会把收到的原样回显出来；录制器看到屏幕安静下来就落一个锚点。
        rec.note_screen("hi", at(50));
        rec.tick(at(600));
        let source = to_script(&rec.finish(at(700)));

        let mut run = ScriptRun::start(pool.clone(), id, source, RunOptions::default())
            .expect("起脚本");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let outcome = loop {
            if let Some(outcome) = run.try_outcome() {
                break outcome;
            }
            assert!(std::time::Instant::now() < deadline, "录出来的脚本跑不完");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(
            matches!(outcome, Outcome::Done(_)),
            "生成物必须能直接跑，实际：{outcome:?}"
        );

        pool.close(id);
    }

    #[test]
    fn every_control_character_survives_the_literal() {
        // 录到的字节可能是任何东西（ESC、回车、引号）；生成物必须是合法源码。
        assert_eq!(rhai_literal("a\r\n\t"), "\"a\\r\\n\\t\"");
        assert_eq!(rhai_literal("\x1b[A"), "\"\\u001b[A\"");
        assert_eq!(rhai_literal("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(rhai_literal("c:\\tmp"), "\"c:\\\\tmp\"");
        assert_eq!(rhai_literal("\x7f"), "\"\\u007f\"");
        // 中文原样留着：它是正常文本，转成 `\u` 只会让脚本没法读。
        assert_eq!(rhai_literal("你好"), "\"你好\"");
    }

    /// 落库名字按 `recorded` / `recorded-2` 往后找空位：第二次录制不能盖掉第一次。
    /// 直接给临时目录，不碰 `WINDTERM_PROFILE_DIR`——那是进程级变量，与
    /// `with_temp_profile` 的用例并发时会互相看见对方的档案目录。
    #[test]
    fn save_to_library_never_overwrites_a_previous_recording() {
        let dir = std::env::temp_dir().join(format!(
            "zterm-recorder-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);

        let first = save_to_library_in(&dir, "// first\n").expect("第一次落库");
        let second = save_to_library_in(&dir, "// second\n").expect("第二次落库");
        assert_eq!(first.file_name().and_then(|s| s.to_str()), Some("recorded.rhai"));
        assert_eq!(second.file_name().and_then(|s| s.to_str()), Some("recorded-2.rhai"));
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "// first\n");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "// second\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_generated_script_has_one_statement_per_step() {
        let steps = vec![
            Step::Send("ls\r".to_string()),
            Step::Wait("$ ".to_string()),
        ];
        let text = to_script(&steps);
        assert!(text.contains("write(\"ls\\r\");"), "{text}");
        assert!(text.contains("read_until(\"$ \", 10);"), "{text}");
        // 注释只有文件头一行，不该每步都插一句。
        assert_eq!(text.lines().count(), 3, "{text}");
    }
}
