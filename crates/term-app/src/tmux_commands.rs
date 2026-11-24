//! tmux 控制模式的反向命令族：纯字符串生成，投影层只负责写进 PTY。
//!
//! 从 `tmux.rs` 拆出以为守住 500 代码行门禁；公共 API 经 `pub use` 不变。
//! 所有变体都返回安全语法：能注入的字符已转义或改走十六进制，调用方不需要
//! （也不应该）再拼一次。

/// 本地 → 远端的反向控制命令（`tmux -CC` 里发给会话的那些字符串）。
/// 纯字符串生成，投影层负责写进 PTY。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneDir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    SplitHorizontal,
    SplitVertical,
    /// 官方 page/Split-Pane：`Alt+-` 向右、`Alt+Shift+-` 向左、
    /// `Alt+|` 向下、`Alt+Shift+|` 向上。tmux 的 `-b` = 在左侧/上方新建。
    SplitPaneLeft,
    SplitPaneUp,
    /// 官方 page/Toggle-Pane-Zoom：无目标，作用于当前窗格。
    TogglePaneZoom,
    SelectPane(u32),
    /// 文档里的「Alt+方向键 / Mux: Select Pane Left…」：无目标，作用于当前窗格。
    SelectPaneDirection(PaneDir),
    KillPane(u32),
    /// `Mux: Kill Pane`：无目标时 tmux 自己杀当前窗格，不需要我们先解析窗格号。
    KillPaneActive,
    /// `Mux: Rename Pane`：带目标的窗格重命名（名称来自面板输入框，安全转义）。
    RenamePane { pane: u32, name: String },
    /// `Tmux: Synchronize Panes`：tmux 的 window 选项，省略值即切换。
    SynchronizePanes,
    NewWindow,
    SelectWindow(u32),
    /// 官方 Common Keyboard Shortcuts：`Alt+[` / `Alt+]` 切换 window。
    SelectWindowPrevious,
    SelectWindowNext,
    RenameWindow(String),
    RenameSession(String),
    KillWindow,
    KillSession,
    SwapPane,
    SwapWindow,
    /// 官方 page/Move-Pane-To-New-Window：tmux 的 `break-pane`。
    MovePaneToNewWindow,
    /// 官方 page/Select-Layout 的五种 tmux 内建布局（另两种 mirrored 变体
    /// 不是 tmux 的 layout 名，WindTerm 在 UI 侧改窗格顺序实现；不伪造控制行）。
    SelectLayout(&'static str),
    Detach,
    Refresh,
    /// `refresh-client -C <cols>x<rows>`：tmux 的 client resize 入口（整窗）。
    Resize { cols: u16, rows: u16 },
    /// `resize-pane -t %N -x N -y M`：真正改某个窗格尺寸，不是整窗。
    ResizePane { pane: u32, cols: u16, rows: u16 },
    /// `send-keys -l <text>`：按字面发送文本。内容来自用户，**必须**引号转义。
    SendText { pane: u32, text: String },
    /// `send-keys -H <hex…>`：逐字节十六进制，二进制不会被 UTF-8 截断、也无从注入。
    SendBytes { pane: u32, data: Vec<u8> },
}

impl Command {
    /// 最终要写进 PTY 的一行。**所有变体都返回安全语法**：能注入的字符已经
    /// 转义或改走十六进制，调用方不需要（也不应该）再拼一次。
    pub fn line(&self) -> String {
        match self {
            Command::SplitHorizontal => "split-window -h".to_string(),
            Command::SplitVertical => "split-window -v".to_string(),
            Command::SplitPaneLeft => "split-window -h -b".to_string(),
            Command::SplitPaneUp => "split-window -v -b".to_string(),
            Command::TogglePaneZoom => "resize-pane -Z".to_string(),
            Command::SelectPane(id) => format!("select-pane -t %{id}"),
            Command::SelectPaneDirection(dir) => {
                let flag = match dir {
                    PaneDir::Left => 'L',
                    PaneDir::Right => 'R',
                    PaneDir::Up => 'U',
                    PaneDir::Down => 'D',
                };
                format!("select-pane -{flag}")
            }
            Command::KillPane(id) => format!("kill-pane -t %{id}"),
            Command::KillPaneActive => "kill-pane".to_string(),
            Command::RenamePane { pane, name } => {
                format!("rename-pane -t %{pane} {}", quote_if_needed(&no_control(name)))
            }
            Command::SynchronizePanes => "set-window-option synchronize-panes".to_string(),
            Command::NewWindow => "new-window".to_string(),
            Command::SelectWindow(id) => format!("select-window -t @{id}"),
            Command::SelectWindowPrevious => "select-window -p".to_string(),
            Command::SelectWindowNext => "select-window -n".to_string(),
            Command::RenameWindow(name) => format!("rename-window {}", quote_if_needed(&no_control(name))),
            Command::RenameSession(name) => format!("rename-session {}", quote_if_needed(&no_control(name))),
            Command::KillWindow => "kill-window".to_string(),
            Command::KillSession => "kill-session".to_string(),
            Command::SwapPane => "swap-pane".to_string(),
            Command::SwapWindow => "swap-window".to_string(),
            Command::MovePaneToNewWindow => "break-pane".to_string(),
            Command::SelectLayout(name) => format!("select-layout {name}"),
            Command::Detach => "detach-client".to_string(),
            Command::Refresh => "refresh-client".to_string(),
            Command::Resize { cols, rows } => format!("refresh-client -C {cols}x{rows}"),
            Command::ResizePane { pane, cols, rows } => {
                format!("resize-pane -t %{pane} -x {cols} -y {rows}")
            }
            Command::SendText { pane, text } => {
                // 换行等控制字符本身就是命令分隔符，不能出现在一条控制命令里；
                // 一旦有就退到十六进制路径，和二进制同一处理。
                if text.chars().any(char::is_control) {
                    send_bytes_line(*pane, text.as_bytes())
                } else {
                    format!("send-keys -t %{pane} -l {}", quote(text))
                }
            }
            Command::SendBytes { pane, data } => send_bytes_line(*pane, data),
        }
    }

    /// 引号转义的显式入口。`line()` 已经全程安全，这里保留给「我要确认这条命令
    /// 经过了转义」的调用点，语义上是 `line()` 的别名。
    pub fn quoted_line(&self) -> String {
        self.line()
    }
}

/// 总是加双引号：`send-keys -l` 的一个参数就是一整段文本，不加引号时
/// 空格会把内容拆成多个 key、`;` 会被 tmux 当成命令分隔符（注入面）。
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// 只有真的需要时才加引号，保持 `rename-window prod` 这种可读形式。
/// 单引号也算：tmux 的命令行里 `'…'` 同样是引号，留着会把后面的参数吞掉。
fn quote_if_needed(text: &str) -> String {
    let needs = text.chars().any(|c| c.is_whitespace() || matches!(c, ';' | '"' | '\'' | '\\'));
    if needs {
        quote(text)
    } else {
        text.to_string()
    }
}

/// 窗口名没有合法的控制字符语义，而换行会**截断**一条控制命令（等于注入），
/// 所以这里一律换成空格；`send-keys` 则不走这条路径（它有能力发任意字节）。
fn no_control(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

fn send_bytes_line(pane: u32, data: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = format!("send-keys -t %{pane} -H");
    for b in data {
        // `write!` 到 String 不会失败；忽略返回值而非 unwrap，符合「外部输入不 panic」。
        let _ = write!(out, " {b:02x}");
    }
    out
}

/// 文本发送路径：内容按字面送达（空格、引号、`;` 不会变成第二条命令）。
pub fn send_text(pane: u32, text: &str) -> Command {
    Command::SendText { pane, text: text.to_string() }
}

/// 裸写发送路径：能当 UTF-8 解释的走文本，否则逐字节十六进制。
/// 二进制（比如鼠标上报、粘贴的压缩包）不会在 UTF-8 边界上被截断或替换。
pub fn send_keys_raw(pane: u32, bytes: &[u8]) -> Command {
    match std::str::from_utf8(bytes) {
        Ok(text) => send_text(pane, text),
        Err(_) => Command::SendBytes { pane, data: bytes.to_vec() },
    }
}

