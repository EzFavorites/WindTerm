//! Sender 的类型与**纯函数**：模式、目标、切片、十六进制解析。
//!
//! 拆出来只有一个理由——行数上限。这里一样都不碰 `egui`：`Target` 是
//! 一个五项枚举，`units()` / `parse_hex()` 是字符串进、字符串出的纯函数，
//! 所以它们可以直接单测，不必为了测一次「按字符切片」去造一个窗口。

use term_core::session_runtime::SessionId;

/// 发给谁。五项与视觉基线展开的那张表一一对应。
///
/// `Window` 与 `AllWindows` 在本实现里是同一批标签：进程只有一个窗口，
/// 多窗口那句是沿用菜单文字口径。保留两个词是因为菜单文字要对得上，
/// 语义差别等真开了多窗口再补。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum Target {
    Current,
    Group,
    Window,
    AllWindows,
    Selected,
}

impl Target {
    pub(in crate::ui) const ALL: [Target; 5] = [
        Target::Current,
        Target::Group,
        Target::Window,
        Target::AllWindows,
        Target::Selected,
    ];

    pub(in crate::ui) fn title(self) -> &'static str {
        match self {
            Target::Current => "Current Session",
            Target::Group => "Sessions In Current Group",
            Target::Window => "Sessions In Current Window",
            Target::AllWindows => "Sessions In All Windows",
            Target::Selected => "Selected Sessions",
        }
    }
}

/// `Target::Selected` 右边那个多选下拉在**未展开**时显示什么。
///
/// 名字多于两个就收成「前两个 + ` +N`」，再按**显示宽度**截到 28 个半角位：
/// 那一格宽 229 点、字体 12 点，塞进五个会话名只会被硬切成半个字，反倒不如
/// 「web-1, db +3」——一眼能数出勾了几个。
///
/// 为什么按显示宽度而不是字符数：中文一个字占两格，按字符数截 26 个字会
/// 直接顶出框外。按宽度算，两种语言都落在同一个上限里。
pub(in crate::ui) fn picked_title(names: &[String], empty: &str) -> String {
    const SHOWN: usize = 2;
    const MAX_W: usize = 28;
    if names.is_empty() {
        return empty.to_string();
    }
    let mut out = names
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > SHOWN {
        out.push_str(&format!(" +{}", names.len() - SHOWN));
    }
    if display_width(&out) > MAX_W {
        // 省略号按全角算（它在本界面的字体里就是两格），所以正文只截到 MAX_W-2。
        out = format!("{}…", clip_to_width(&out, MAX_W - 2));
    }
    out
}

/// 显示宽度：ASCII 算 1，其余按全角算 2。只用来判断「这一串会不会撑破下拉框」。
fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// 按显示宽度截断，**绝不切开一个 UTF-8 字符**。
fn clip_to_width(s: &str, max: usize) -> String {
    let mut w = 0usize;
    let mut out = String::new();
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw > max {
            break;
        }
        w += cw;
        out.push(c);
    }
    out
}

fn char_width(c: char) -> usize {
    if c.is_ascii() {
        1
    } else {
        2
    }
}

/// 勾选集 → 标签序号（按标签顺序，去重）。
///
/// 键是恒在身份 [`SessionId`]，不是名字也不是序号：勾选表里那些已经关掉的
/// 标签在这里**自然跳过**——所以关标签时不需要清理勾选表，也不会出现
/// 「序号整体前移，勾到别人头上」。
pub(in crate::ui) fn picked_indices(
    picked: &[SessionId],
    identities: &[SessionId],
) -> Vec<usize> {
    identities
        .iter()
        .enumerate()
        .filter(|(_, id)| picked.contains(*id))
        .map(|(i, _)| i)
        .collect()
}

/// 一次正在跑的定时发送。
///
/// `due` 是**绝对时刻**（`ctx.input(|i| i.time)` 的秒数），不是「还剩多久」：
/// 每帧累加会随帧率漂移，绝对时刻不会。
pub(in crate::ui) struct Run {
    /// 已经是**解析好**的片：定时发送期间不再有任何解析，因此不可能
    /// 「跑到一半才发现第三片写错了」。
    pub pieces: Vec<Piece>,
    pub at: usize,
    pub due: f64,
    pub gap: f64,
}

/// Sender 的全部控件状态（投影层，不进 `ViewState`）。
pub(in crate::ui) struct SenderState {
    pub buf: String,
    /// `false` = Text，`true` = Hex。
    pub hex: bool,
    /// `false` = 按行，`true` = 按字符。
    pub by_char: bool,
    pub count: u32,
    pub interval: f32,
    pub target: Target,
    /// `Target::Selected` 时**显式勾选**的会话。
    ///
    /// 键是 `Tab::identity` 而不是名字：两条同名标签是两条会话，按名字勾会把
    /// 它们一起发（而用户只看见自己点了一个）。也不按标签序号：关掉一个标签，
    /// 后面所有序号整体前移，勾选集会在用户背后换人。
    ///
    /// 已关掉的标签自然失效——`sender_targets` 只取**仍在册**的那些，
    /// 所以这里不需要在关标签时清理，也不怕留一条指向已消失会话的键。
    pub picked: Vec<SessionId>,
    pub run: Option<Run>,
    /// 一次性提示（十六进制写错 / 空缓冲）。
    pub note: Option<String>,
    /// 「下一帧把光标挪到末尾」的一次性请求（`＋ / − / ×` 之后）。
    pub caret_end: bool,
}

impl Default for SenderState {
    fn default() -> Self {
        Self {
            buf: String::new(),
            hex: false,
            by_char: false,
            count: 1,
            // 视觉基线就是这个值：`1.00s`。
            interval: 1.0,
            target: Target::Current,
            picked: Vec::new(),
            run: None,
            note: None,
            caret_end: false,
        }
    }
}

/// 按模式把一个缓冲切成单片。
///
/// - Hex：按空白分片（`41 42` 是两片字节）；
/// - 按字符：一个字符一片，**不补 `\n`**（每字补一个回车就等于逐字清屏）；
/// - 按行：一行一片，**每片自带 `\n`**（发送时它会被 `paste_bytes_for`
///   归一成 `\r`，等于替用户按了一次回车）。末尾那个 `\n` 不算额外一行。
///
/// Hex 那一支**已经不再是发送路径的入口**：`plan()` 换成按行 / 按字节切，
/// 因为「按空白切」表达不出 `By: Line / Char` 在 hex 下的区别（那个开关
/// 一度是死的）。这里保留原样，是因为文本路径一位都不能变，而它的四条
/// 文本测试正是「零回归」的判据。
pub(in crate::ui) fn units(buf: &str, hex: bool, by_char: bool) -> Vec<String> {
    if hex {
        return buf.split_whitespace().map(|t| t.to_string()).collect();
    }
    if by_char {
        return buf.chars().map(|c| c.to_string()).collect();
    }
    let body = buf.trim_end_matches('\n');
    if body.is_empty() {
        Vec::new()
    } else {
        body.split('\n').map(|l| format!("{l}\n")).collect()
    }
}

/// 十六进制串 → 字节。奇数位、非十六进制字符都算错（返回 `None`）。
///
/// 允许 `41` 与 `4142` 两种写法：短串一个字节、长串一串字节，都是日常写法。
pub(in crate::ui) fn parse_hex(s: &str) -> Option<Vec<u8>> {
    let t = s.trim();
    if t.is_empty() || !t.len().is_multiple_of(2) {
        return None;
    }
    let b = t.as_bytes();
    let mut out = Vec::with_capacity(t.len() / 2);
    for pair in b.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
    }
    Some(out)
}

/// 一片要写出去的东西：文本（走终端编码）还是原始字节（hex 模式）。
///
/// 分成两个变体而不是「统一的 `Vec<u8>` + 一个 bool」：文本片必须经过
/// `paste_bytes_for`（换行归一 + 括号粘贴），而字节片**绝不能**被那样处理
/// ——它就是要原样上线。类型分开，就不会有「不小心对二进制做了 CRLF 归一」。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) enum Piece {
    Text(String),
    Bytes(Vec<u8>),
}

/// 计划阶段的错误。**一旦返回它，一个字节都还没写出去**。
///
/// `line` 是**编辑器里的行号**（1 起，空行也占号），`text` 是写错的那个词原文
/// ——两者都要进提示：只说「十六进制写错了」等于让用户自己在几百个字节里找一个坏字。
///
/// 为什么是行号而不是片号：`By: Char` 下一片只有一个字节，片号在那个模式下
/// 既指错了对象也数错了单位；而行号在两种模式下都是同一个东西，也是用户唯一
/// 能直接对上眼的东西（他要照提示去改的那一行）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) enum PlanError {
    /// 位数是奇数（`4`、`41,42` 都落这里）。
    OddLength { line: usize, text: String },
    /// 偶数位但不是十六进制（`zz`、`0x41`）。
    NotHex { line: usize, text: String },
    /// 文本模式的转义写错（`\x` 位数不够等）。
    Escape { message: String },
}

impl PlanError {
    /// 给用户看的一句话（`tools_sender_row.rs` 的提示区直接画它）。
    pub(in crate::ui) fn note(&self) -> String {
        match self {
            PlanError::OddLength { line, text } => {
                format!("第 {line} 行 \"{text}\" 不是十六进制（需要偶数位）")
            }
            PlanError::NotHex { line, text } => format!("第 {line} 行 \"{text}\" 不是十六进制"),
            PlanError::Escape { message } => format!("转义写错了：{message}"),
        }
    }
}

/// Hex 的「行」：与文本按行模式同一条切法（末尾换行不算额外一行），
/// 但空行 / 全空白行直接丢掉——它们在 hex 里不代表任何字节，
/// 留着只会凭空多出一片「发 0 字节」。
///
/// **行号在第一件事就定下来**：先 `enumerate()` 数出 `i + 1`，**再**过滤空行。
/// 反过来（先 `filter` 再 `enumerate`）数出来的是「第几个非空行」，
/// 于是 `41\n\n4` 里写错的 `4`（第 3 行）会被报成「第 2 片」——用户照着提示
/// 去第 2 行找一个不存在的错，这种提示比没有提示更坏。
///
/// 空行本身仍然不产生片（那是 `plan` 的语义：空行不代表任何字节），
/// 它只是**占一个行号**。
fn hex_lines(buf: &str) -> Vec<(usize, &str)> {
    buf.trim_end_matches('\n')
        .split('\n')
        .enumerate()
        .map(|(i, line)| (i + 1, line))
        .filter(|(_, line)| !line.trim().is_empty())
        .collect()
}

/// 一个空白分隔的十六进制词 → 字节。`line` 只用来给用户指路（它在哪一行）。
fn hex_token(token: &str, line: usize) -> Result<Vec<u8>, PlanError> {
    let t = token.trim();
    if !t.len().is_multiple_of(2) {
        return Err(PlanError::OddLength {
            line,
            text: t.to_string(),
        });
    }
    parse_hex(t).ok_or_else(|| PlanError::NotHex {
        line,
        text: t.to_string(),
    })
}

/// 把整个缓冲**一次算完**：要么给出完整的片序列，要么给出第一个错误。
///
/// 为什么必须整体算：旧的发送路径是「逐片解析 + 逐片写出」，第 3 片解析失败时
/// 前 2 片已经进设备了——在串口上那意味着对面收到了半条命令。计划与写出分成
/// 两步之后，校验失败时设备那边一个字节都没动。
///
/// - `hex = false`：完全复用 [`units`]，文本路径零回归；
/// - `hex = true, by_char = false`：**一整行一片**，片内是这一行解析出的全部
///   字节（一行 = 一个报文，按间隔一片片发出去）；
/// - `hex = true, by_char = true`：**一个字节一片**，片数 = 字节数。
///   这就是 `By: Char` 在 hex 下终于有了真语义（它以前被 `units` 的早退
///   忽略掉，是个死开关）。
pub(in crate::ui) fn plan(buf: &str, hex: bool, by_char: bool) -> Result<Vec<Piece>, PlanError> {
    if !hex {
        return units(buf, false, by_char)
            .into_iter()
            .map(|unit| {
                if !term_core::send_escapes::has_escape(&unit) {
                    return Ok(Piece::Text(unit));
                }
                term_core::send_escapes::decode(&unit)
                    .map(Piece::Bytes)
                    .map_err(|e| PlanError::Escape { message: e.to_string() })
            })
            .collect();
    }
    if by_char {
        // 先按行定位（错在哪一行要说得准），再逐词解析，最后摊成「一字节一片」。
        let mut out = Vec::new();
        for (line, text) in hex_lines(buf) {
            for token in text.split_whitespace() {
                for b in hex_token(token, line)? {
                    out.push(Piece::Bytes(vec![b]));
                }
            }
        }
        return Ok(out);
    }
    hex_lines(buf)
        .into_iter()
        .map(|(line, text)| {
            let mut bytes = Vec::new();
            for token in text.split_whitespace() {
                bytes.extend(hex_token(token, line)?);
            }
            Ok(Piece::Bytes(bytes))
        })
        .collect()
}

/// 每片重复 `count` 次（工具行的 `Count:` 是「每一片发几次」）。
pub(in crate::ui) fn repeat(pieces: &[Piece], count: u32) -> Vec<Piece> {
    let count = count.max(1) as usize;
    let mut out = Vec::with_capacity(pieces.len() * count);
    for piece in pieces {
        for _ in 0..count {
            out.push(piece.clone());
        }
    }
    out
}

/// 计划 → 逐片写出，返回写了几片。
///
/// 抽成函数不是为了复用，而是为了让「**先校验、后动笔**」这条顺序有地方被
/// 钉住：`plan()` 出错时返回的 `Err` 在闭包之前就短路掉了，所以调用方拿到的
/// 错误一定伴随「零次写出」。单测用一个计数的闭包把这件事量出来。
pub(in crate::ui) fn plan_then_write(
    buf: &str,
    hex: bool,
    by_char: bool,
    count: u32,
    mut write: impl FnMut(&Piece),
) -> Result<usize, PlanError> {
    let pieces = repeat(&plan(buf, hex, by_char)?, count);
    for piece in &pieces {
        write(piece);
    }
    Ok(pieces.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing_takes_short_and_long_forms() {
        assert_eq!(parse_hex("41"), Some(vec![0x41]));
        assert_eq!(parse_hex("410a"), Some(vec![0x41, 0x0a]));
        assert_eq!(parse_hex(" 7f "), Some(vec![0x7f]));
    }

    #[test]
    fn hex_parsing_rejects_odd_and_non_hex() {
        assert_eq!(parse_hex("4"), None);
        assert_eq!(parse_hex("zz"), None);
        assert_eq!(parse_hex(""), None);
        // `0x` 前缀不认：切片已经按空白分过，再留一个前缀只会让两个写法人猜。
        assert_eq!(parse_hex("0x41"), None);
    }

    #[test]
    fn every_target_has_its_own_title() {
        let mut seen = Vec::new();
        for t in Target::ALL {
            assert!(!t.title().is_empty());
            assert!(!seen.contains(&t.title()), "{} 与别人同名", t.title());
            seen.push(t.title());
        }
        assert_eq!(Target::ALL.len(), 5);
    }

    /// 勾选集 → 标签序号：按**标签顺序**（不是勾选顺序）落地，已关掉的跳过。
    ///
    /// 这三条正是「指定会话」最容易出错的地方，所以钉成纯函数：
    /// 顺序跟着标签走（发送次序与用户看到的标签条一致），失效的键自动略过
    /// （不必在关标签时清理），重复勾选也只发一次。
    #[test]
    fn the_picked_set_maps_to_tab_indices() {
        assert_eq!(picked_indices(&[3, 1], &[1, 2, 3]), vec![0, 2]);
        assert_eq!(picked_indices(&[9], &[1, 2]), Vec::<usize>::new());
        assert_eq!(picked_indices(&[], &[1, 2]), Vec::<usize>::new());
        assert_eq!(picked_indices(&[2, 2], &[1, 2]), vec![1], "重复勾只发一次");
    }

    /// 多选下拉面上的字：空集给词、多于两个收成 `+N`、超长按字符截断。
    ///
    /// 三条都是**显示**规则，但它们决定用户能不能看出「到底勾了几个」，
    /// 所以钉在这里而不是扔进绘制代码。
    #[test]
    fn the_picked_title_folds_and_truncates() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(picked_title(&names(&[]), "None"), "None");
        assert_eq!(picked_title(&names(&["a"]), "None"), "a");
        assert_eq!(picked_title(&names(&["a", "b"]), "None"), "a, b");
        assert_eq!(picked_title(&names(&["a", "b", "c"]), "None"), "a, b +1");
    }

    /// 截断按**显示宽度**走：中文不会被切成半个 UTF-8 字节，也不会顶出框外，
    /// 且被截断时一定带省略号。
    #[test]
    fn the_picked_title_truncates_to_the_box_width() {
        let long = vec!["x".repeat(40)];
        let t = picked_title(&long, "None");
        assert!(display_width(&t) <= 28, "超宽了：{t}");
        assert!(t.ends_with('…'), "被截断就要看得出来：{t}");

        // 中文一个字两格：13 个汉字（26 宽）加省略号刚好落在上限内。
        let cjk = vec!["会话名称很长的中文标签".repeat(3)];
        let t = picked_title(&cjk, "None");
        assert!(display_width(&t) <= 28, "超宽了：{t}");
        assert!(t.ends_with('…'));
        // 截断点不许落在半个字上——`chars()` 能走完就说明没切坏 UTF-8。
        assert!(t.chars().count() >= 2);
    }

    #[test]
    fn the_plan_splits_by_line_and_keeps_the_newline() {
        // 末尾那个 '\n' 不额外算一行：`a\nb\n` 是两行，不是三行。
        assert_eq!(units("a\nb\n", false, false), vec!["a\n", "b\n"]);
        assert_eq!(units("a\nb", false, false), vec!["a\n", "b\n"]);
        assert_eq!(units("", false, false), Vec::<String>::new());
        assert_eq!(units("\n\n", false, false), Vec::<String>::new());
    }

    #[test]
    fn the_plan_splits_by_char_without_adding_newlines() {
        assert_eq!(units("ab", false, true), vec!["a", "b"]);
        assert_eq!(units("中文", false, true), vec!["中", "文"]);
    }

    #[test]
    fn the_plan_splits_hex_by_whitespace() {
        assert_eq!(units("41 42\n0a", true, false), vec!["41", "42", "0a"]);
        assert_eq!(units("", true, false), Vec::<String>::new());
    }

    // ---- hex 发送的三个洞（死开关 / 分片 / 原子性） ----

    /// 把片序列摊平成字节，用来比对「同一段 hex 的不同写法」是否等价。
    fn bytes_of(pieces: &[Piece]) -> Vec<u8> {
        let mut out = Vec::new();
        for p in pieces {
            match p {
                Piece::Bytes(b) => out.extend_from_slice(b),
                Piece::Text(_) => panic!("hex 模式下不该出现文本片"),
            }
        }
        out
    }

    /// 空格分隔与连写是同一个意思：`41 42` == `4142`。
    ///
    /// 这条钉的是「按行一片」那一支：一行里的多个词要在**同一片**里拼起来，
    /// 而不是被当成两片——否则 `Count` / `Interval` 的语义会随写法变化。
    #[test]
    fn a_hex_line_parses_the_same_with_or_without_spaces() {
        let a = plan("41 42", true, false).expect("空格写法");
        let b = plan("4142", true, false).expect("连写");
        assert_eq!(bytes_of(&a), vec![0x41, 0x42]);
        assert_eq!(bytes_of(&b), vec![0x41, 0x42]);
        assert_eq!(a, b, "两种写法要给出同一份计划");
        assert_eq!(a.len(), 1, "一整行是一片");
    }

    /// 写错要指出「第几行、哪个词」，并且整份计划作废。
    ///
    /// 单位是**行**不是片：`By: Char` 下一片只有一个字节，用片号指路在那个模式下
    /// 既指错对象也数错单位；行号在两种模式下都是同一个东西。
    #[test]
    fn a_bad_hex_piece_names_itself_in_the_note() {
        let err = plan("4 41", true, false).expect_err("奇数位必须失败");
        let note = err.note();
        assert!(note.contains("第 1 行"), "提示要指行号：{note}");
        assert!(!note.contains("片"), "单位是行，不是片：{note}");
        assert!(note.contains("\"4\""), "提示要带原文：{note}");
        assert_eq!(
            err,
            PlanError::OddLength {
                line: 1,
                text: "4".to_string()
            }
        );
    }

    /// **空行占行号、但不产生片**：报错行号必须与编辑器对得上。
    ///
    /// 这条挡的是「先丢空行、后 `enumerate()`」那个错法：那样数出来是「第几个
    /// 非空行」，`41\n\n4` 里写错的 `4`（第 3 行）会被报成「第 2 行」——用户照着
    /// 提示去第 2 行找一个不存在的错。同一份缓冲里合法那一半仍只有 2 片。
    #[test]
    fn a_blank_line_does_not_shift_the_reported_line() {
        let err = plan("41\n\n4", true, false).expect_err("第 3 行是奇数位");
        assert_eq!(
            err,
            PlanError::OddLength {
                line: 3,
                text: "4".to_string()
            },
            "行号要按编辑器数（41 在第 1 行、空行第 2 行、4 在第 3 行）"
        );
        assert!(err.note().contains("第 3 行"), "提示里的行号也得是 3");

        // 空行不产生片：合法的那两行仍是 2 片。
        let pieces = plan("41\n\n43", true, false).expect("两行合法");
        assert_eq!(
            pieces,
            vec![Piece::Bytes(vec![0x41]), Piece::Bytes(vec![0x43])]
        );
        assert_eq!(pieces.len(), 2, "中间那个空行不额外造片");
    }

    /// 大小写不敏感；`0x` 前缀与逗号分隔都不认（各写一套只会让人猜）。
    #[test]
    fn hex_case_is_ignored_and_stray_syntax_is_refused() {
        assert_eq!(
            bytes_of(&plan("4a 4A", true, false).expect("大小写混写")),
            bytes_of(&plan("4a4a", true, false).expect("连写"))
        );
        assert!(plan("0x41", true, false).is_err(), "前缀不认");
        assert!(plan("41,42", true, false).is_err(), "逗号不是分隔符");
    }

    /// **原子性**：计划失败时写出次数为 0。
    ///
    /// 旧路径是逐片 parse + 逐片 write，第 3 片失败时前 2 片已经进设备了。
    /// 这里用计数的闭包量出「先校验、后动笔」确实成立。
    #[test]
    fn a_failed_plan_writes_nothing_at_all() {
        let mut writes = 0usize;
        let outcome = plan_then_write("41 4 43", true, false, 1, |_| writes += 1);
        assert!(outcome.is_err(), "写错必须报错");
        assert_eq!(writes, 0, "计划失败时一个字节都不许写");

        // 反面对照：同一份闭包在合法输入上确实被调用了。
        let mut ok_writes = 0usize;
        let n = plan_then_write("4142", true, false, 2, |_| ok_writes += 1).expect("合法输入");
        assert_eq!(n, 2, "Count=2 时同一片写两次");
        assert_eq!(ok_writes, 2);
    }

    /// `By: Char` 在 hex 下拿到真语义：一个字节一片。
    ///
    /// 这是那个洞的正题——`units()` 在 hex 分支早退，`by_char` 因此被忽略，
    /// 界面上一颗点不动的开关。现在片数 = 字节数。
    #[test]
    fn hex_by_char_makes_one_piece_per_byte() {
        let pieces = plan("4142", true, true).expect("两字节");
        assert_eq!(
            pieces,
            vec![Piece::Bytes(vec![0x41]), Piece::Bytes(vec![0x42])]
        );
        // 分片方式不同，摊平之后的字节流必须一模一样。
        assert_eq!(
            bytes_of(&pieces),
            bytes_of(&plan("4142", true, false).expect("整行"))
        );
    }

    /// hex 下「一行一片」：两行就是两片，空行不产生空片。
    #[test]
    fn hex_lines_become_pieces_and_blank_lines_are_dropped() {
        let pieces = plan("41 42\n\n43\n", true, false).expect("两行");
        assert_eq!(
            pieces,
            vec![Piece::Bytes(vec![0x41, 0x42]), Piece::Bytes(vec![0x43])]
        );
        assert_eq!(plan("", true, false).expect("空缓冲"), Vec::<Piece>::new());
        assert_eq!(
            plan("\n \n", true, false).expect("全空白"),
            Vec::<Piece>::new(),
            "全空白不该产生「发 0 字节」的片"
        );
    }

    /// 上游转义表在文本模式下生效：`\x03` / `\003` / `\cc` 都是 Ctrl+C。
    #[test]
    fn text_mode_decodes_the_documented_escape_table() {
        assert_eq!(
            plan(r"\x03", false, false).expect("转义"),
            vec![Piece::Bytes(vec![0x03, 0x0A])]
        );
        assert_eq!(
            plan(r"\003", false, false).expect("八进制"),
            vec![Piece::Bytes(vec![0x03, 0x0A])]
        );
        assert_eq!(
            plan(r"\cc", false, false).expect("控制字符"),
            vec![Piece::Bytes(vec![0x03, 0x0A])]
        );
        assert!(plan(r"\x0", false, false).is_err(), "写错的转义不该发出去");
    }

    /// 文本路径**一位没变**：`plan` 在 `hex = false` 时就是 `units` 套一层。
    #[test]
    fn the_text_plan_is_exactly_units() {
        for (buf, by_char) in [("a\nb\n", false), ("ab", true), ("", false)] {
            let want: Vec<Piece> = units(buf, false, by_char).into_iter().map(Piece::Text).collect();
            assert_eq!(plan(buf, false, by_char).expect("文本永远合法"), want);
        }
    }
}
