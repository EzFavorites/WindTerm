//! A real pseudo-terminal.
//!
//! A conventional terminal runs its shell and its `ssh`/`telnet`/`nc`/`cu` clients
//! inside a QPtyDevice; this does the same job with `portable-pty`. A reader
//! thread drains the master into a channel so the UI thread never blocks on a
//! busy shell, and the master's writer is kept for typed input.

use crate::error::SpawnError;
use term_core::session::{Protocol, SessionConfig, SpawnPlan};
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

/// Bytes to pull out of the channel per frame, so a `find /` cannot stall paint.
const FRAME_BUDGET: usize = 256 * 1024;

/// A running session: the pty, the child, and the pipe from the reader thread.
pub struct PtySession {
    pub config: SessionConfig,
    master: Box<dyn MasterPty + Send>,
    writer: Option<Box<dyn Write + Send>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    receiver: Receiver<Vec<u8>>,
    /// Set once the reader thread sees end-of-file.
    pub eof: bool,
    pub exit_status: Option<String>,
    pub started: Instant,
    pub ended_at: Option<Instant>,
    /// `session.connectionTimeout`, in seconds; 0 = wait forever. Only enforced
    /// for remote sessions, after which the connect is aborted.
    connection_timeout: u32,
}

/// 状态文本 → 退出码。`" exited"` 是 0，`" exited with code N"` 是 N，
/// 其余（`" stopped"`、连接超时）没有退出码。
///
/// 生成与解析放在同一个文件里：状态文本是给人看的（状态栏那一行），
/// 而 `desk --headless --exec` 需要的是数字——两处措辞必须一起改。
pub fn exit_code_of(status: &str) -> Option<i32> {
    let text = status.trim();
    if text == "exited" {
        return Some(0);
    }
    text.split("code ").nth(1)?.trim().parse().ok()
}

impl PtySession {
    /// Open a pty and start the program the session's plan describes.
    pub fn spawn(config: SessionConfig, cols: usize, rows: usize) -> Result<Self, SpawnError> {
        let plan = config.spawn_plan()?;
        Self::spawn_with(config, cols, rows, plan)
    }

    /// Same as `spawn`, but with an explicit plan. This is used for the headless
    /// runner: a one-shot `/bin/sh -c <command>` needs no SessionConfig entry.
    pub fn spawn_with(
        config: SessionConfig,
        cols: usize,
        rows: usize,
        plan: SpawnPlan,
    ) -> Result<Self, SpawnError> {
        let (sender, receiver): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = channel();

        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| SpawnError::Pty(e.to_string()))?;

        let mut cmd = match &plan {
            SpawnPlan::LocalShell => CommandBuilder::new_default_prog(),
            SpawnPlan::External(argv) => {
                if argv.is_empty() {
                    return Err(SpawnError::Program("没有可执行的程序（argv 为空）".to_string()));
                }
                let mut cmd = CommandBuilder::new(&argv[0]);
                cmd.args(&argv[1..]);
                cmd
            }
        };
        cmd.env("TERM", config.term.as_str());
        cmd.env("COLORTERM", "truecolor");
        if let Ok(lang) = std::env::var("LANG") {
            cmd.env("LANG", lang);
        } else {
            cmd.env("LANG", "en_US.UTF-8");
        }
        // 本地 Shell 的启动工作目录。空串沿用当前目录（仍留作占位）。
        if config.protocol == Protocol::Shell && !config.working_dir.trim().is_empty() {
            cmd.cwd(config.working_dir.trim());
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| SpawnError::Program(format!("{}: {e}", describe(&plan))))?;
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| SpawnError::Pty(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| SpawnError::Pty(e.to_string()))
            .ok();

        std::thread::spawn(move || pump(reader, sender));

        Ok(Self {
            connection_timeout: config.connection_timeout,
            config,
            master: pair.master,
            writer,
            child,
            receiver,
            eof: false,
            exit_status: None,
            started: Instant::now(),
            ended_at: None,
        })
    }

    /// Everything the shell has written since the last call, capped per frame.
    pub fn drain(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        while out.len() < FRAME_BUDGET {
            match self.receiver.try_recv() {
                Ok(chunk) => out.extend_from_slice(&chunk),
                Err(_) => {
                    self.eof = true;
                    break;
                }
            }
        }
        out
    }

    pub fn send(&mut self, bytes: &[u8]) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        if writer.write_all(bytes).is_ok() {
            let _ = writer.flush();
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let _ = self.master.resize(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    /// Poll the child; records the exit status the first time it is gone.
    pub fn poll(&mut self) -> Option<String> {
        if self.exit_status.is_some() {
            return self.exit_status.clone();
        }
        // `session.connectionTimeout`: give a remote session this long to connect
        // before aborting it, the way a conventional connect dialog does.
        if self.connection_timeout > 0
            && self.config.protocol != term_core::session::Protocol::Shell
            && self.started.elapsed() >= Duration::from_secs(self.connection_timeout as u64)
        {
            self.exit_status = Some(" connection timed out".to_string());
            self.ended_at = Some(Instant::now());
            let _ = self.child.kill();
            return self.exit_status.clone();
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                let text = match status.exit_code() {
                    0 => " exited".to_string(),
                    code => format!(" exited with code {code}"),
                };
                self.exit_status = Some(text);
                self.ended_at = Some(Instant::now());
                self.exit_status.clone()
            }
            Ok(None) => None,
            Err(_) => {
                self.exit_status = Some(" stopped".to_string());
                self.ended_at = Some(Instant::now());
                self.exit_status.clone()
            }
        }
    }

    pub fn finished(&self) -> bool {
        self.exit_status.is_some()
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        // A terminal window that closes takes its shell with it, as in the
        // original: closing the tab sends SIGHUP through the master.
        let _ = self.child.kill();
    }
}

/// Reader thread body: copy the master until it reports end-of-file.
fn pump(mut reader: Box<dyn Read + Send>, sender: Sender<Vec<u8>>) {
    let mut buf = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if sender.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

fn describe(plan: &SpawnPlan) -> String {
    match plan {
        SpawnPlan::LocalShell => "$SHELL".to_string(),
        SpawnPlan::External(argv) => argv.first().cloned().unwrap_or_default(),
    }
}

/// How long to keep a finished session's screen on screen before dropping it.
pub const DEAD_SESSION_GRACE: Duration = Duration::from_secs(30);
#[cfg(test)]
mod tests {
    use super::exit_code_of;

    #[test]
    fn a_clean_exit_has_no_code_in_the_text() {
        assert_eq!(exit_code_of(" exited"), Some(0));
        assert_eq!(exit_code_of("exited "), Some(0));
    }

    #[test]
    fn a_failing_exit_carries_its_number() {
        assert_eq!(exit_code_of(" exited with code 2"), Some(2));
        assert_eq!(exit_code_of(" exited with code 130"), Some(130));
    }

    #[test]
    fn stopping_or_timing_out_has_no_exit_code() {
        assert_eq!(exit_code_of(" stopped"), None);
        assert_eq!(exit_code_of(" connection timed out"), None);
        assert_eq!(exit_code_of(""), None);
    }
}
