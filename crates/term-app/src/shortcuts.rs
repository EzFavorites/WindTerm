//! 快捷键的**匹配与绑定纯层**（数据层）。
//!
//! 三层：
//! - `Chord`：一次按键长什么样（修饰键 + 规范键名）。`text()` 与 `parse_chord()`
//!   互为往返，键名一律折回固定的 `&'static str`，比较零分配。
//! - `Keymap`：和弦 → 命令 ID 的绑定表，带作用域优先级、冲突检测、用户覆盖与
//!   朴素行格式序列化。**本层不认识「有哪些命令」**：命令 ID 是调用方传进来的
//!   不透明字符串，数据层的默认表与用户覆盖因此可以共用一张表而不互相耦合。
//! - `KeySeqState`：两段式键（`Alt+W, Alt+H`）的状态机。单键的判定是
//!   「这一击命中了吗」（`matches`），键序的判定是「这一击在一段等待中的序列里
//!   算什么」（`feed` → `Pending` / `Fire` / `Pass`）。两者并列，谁都不改谁。
//!
//! 没有 UI 依赖、没有 IO、没有全局状态，可单测。
//!
//! `Cmd` 是跨平台写法：macOS 上是 Command，Windows / Linux 上是 Ctrl。
//! 投影层读 egui 的 `modifiers.command` / `modifiers.ctrl`，两者都归一到 `Cmd`。

/// 一次按键的四个修饰键 + 主键名（规范名，如 `P` / `F1` / `ArrowUp` / `Plus`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub cmd: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: &'static str,
}

impl Chord {
    pub fn text(&self) -> String {
        let mut s = String::new();
        if self.cmd {
            s.push_str("Cmd+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        if self.shift {
            s.push_str("Shift+");
        }
        s.push_str(self.key);
        s
    }
}

/// 绑定作用域。`Ord` 的顺序即优先级：`Tab` 最高，`Global` 最低。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    Global,
    Window,
    Tab,
}

/// 一条绑定：谁（作用域）把哪个和弦指给了哪个命令。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub chord: Chord,
    /// 命令 ID，例如 `file.save`。本层只把它当不透明字符串。
    pub command: String,
    pub scope: Scope,
}

/// 和弦 → 命令的绑定表。
///
/// 存储是**追加式**的：同 (作用域, 和弦) 再绑一次不覆盖旧记录，而是追加。
/// 「后加的覆盖先加的」因此体现为解析时取最后一条——这样 `conflicts()` 仍能
/// 看见「同一作用域里同一个键被指给了两个命令」这件事，而不会悄悄消失。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Keymap {
    pub fn new() -> Self {
        Self::default()
    }

    /// 后加的覆盖先加的（用户覆盖默认时调用方先装默认再装覆盖）。
    pub fn bind(&mut self, chord: Chord, command: impl Into<String>, scope: Scope) {
        self.bindings.push(Binding { chord: canonical_chord(chord), command: command.into(), scope });
    }

    /// 解绑某个作用域里的某个键（用户「清除」）。返回是否真的删掉了。
    pub fn unbind(&mut self, chord: Chord, scope: Scope) -> bool {
        let chord = canonical_chord(chord);
        let before = self.bindings.len();
        self.bindings.retain(|b| b.chord != chord || b.scope != scope);
        self.bindings.len() != before
    }

    /// 按优先级解析：`Tab` > `Window` > `Global`；同优先级取**最后**绑定的。
    pub fn resolve(&self, chord: Chord) -> Option<(&str, Scope)> {
        let chord = canonical_chord(chord);
        // `max_by_key` 在并列最大时返回最后一个元素，正好是「后加的赢」。
        let best = self.bindings.iter().filter(|b| b.chord == chord).max_by_key(|b| b.scope)?;
        Some((best.command.as_str(), best.scope))
    }

    /// 某作用域内的全部绑定，按 (scope 降序, `chord.text()` 升序) 排序——顺序稳定才能上基线。
    pub fn bindings(&self, scope: Scope) -> Vec<&Binding> {
        let mut rows: Vec<&Binding> = self.bindings.iter().filter(|b| b.scope == scope).collect();
        rows.sort_by(|a, b| b.scope.cmp(&a.scope).then_with(|| a.chord.text().cmp(&b.chord.text())));
        rows
    }

    /// 全部绑定（各作用域），按 (scope 升序, `chord.text()` 升序) 排序。
    /// 序列化与往返断言都建立在它上面。
    pub fn bindings_all(&self) -> Vec<Binding> {
        let mut rows = self.bindings.clone();
        rows.sort_by(|a, b| a.scope.cmp(&b.scope).then_with(|| a.chord.text().cmp(&b.chord.text())));
        rows
    }

    /// 冲突：同一作用域内同一和弦绑了不同命令，返回 (和弦, 先前命令, 最后命令)，按和弦文本排序。
    /// 不同作用域的重叠**不算**冲突——那正是优先级要解决的。
    pub fn conflicts(&self) -> Vec<(Chord, String, String)> {
        let mut seen: Vec<(Chord, Scope)> = Vec::new();
        let mut out = Vec::new();
        for b in &self.bindings {
            if seen.contains(&(b.chord, b.scope)) {
                continue;
            }
            seen.push((b.chord, b.scope));
            let mut group = self.bindings.iter().filter(|o| o.chord == b.chord && o.scope == b.scope);
            let Some(last) = group.next_back() else { continue };
            if let Some(other) = group.find(|o| o.command != last.command) {
                out.push((b.chord, other.command.clone(), last.command.clone()));
            }
        }
        out.sort_by(|a, b| a.0.text().cmp(&b.0.text()).then_with(|| a.1.cmp(&b.1)));
        out
    }

    /// 全部绑定的数量（含各作用域）。
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// 朴素行格式：`Cmd+Shift+P = palette.open @global`，按 (scope 升序, 和弦文本) 输出，末尾换行。
    /// 空表输出空串。
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        for b in self.bindings_all() {
            s.push_str(&b.chord.text());
            s.push_str(" = ");
            s.push_str(&b.command);
            s.push_str(" @");
            s.push_str(scope_name(b.scope));
            s.push('\n');
        }
        s
    }

    /// 外部输入的容错解析：跳过空行与 `#` 注释，容忍 `=` 两侧空格；
    /// 未知作用域视为 `Global`，和弦或命令解析失败的行整行丢弃（绝不 panic）。
    pub fn from_text(text: &str) -> Keymap {
        let mut map = Keymap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((left, right)) = line.split_once('=') else { continue };
            let Some(chord) = parse_chord(left) else { continue };
            let right = right.trim();
            let (command, scope) = match right.rsplit_once('@') {
                Some((cmd, tag)) if !tag.trim().is_empty() => {
                    (cmd.trim(), parse_scope(tag).unwrap_or(Scope::Global))
                }
                _ => (right, Scope::Global),
            };
            if command.is_empty() {
                continue;
            }
            map.bind(chord, command, scope);
        }
        map
    }
}

/// 表里只存规范键名：`" "` 折成 `Space`。认不出的键名原样保留——
/// 投影层可能送来不在本表里的键，那时宁可存下来也不要丢。
fn canonical_chord(chord: Chord) -> Chord {
    match canonical_key(chord.key) {
        Some(key) => Chord { key, ..chord },
        None => chord,
    }
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Global => "global",
        Scope::Window => "window",
        Scope::Tab => "tab",
    }
}

fn parse_scope(tag: &str) -> Option<Scope> {
    let tag = tag.trim();
    if tag.eq_ignore_ascii_case("global") {
        Some(Scope::Global)
    } else if tag.eq_ignore_ascii_case("window") {
        Some(Scope::Window)
    } else if tag.eq_ignore_ascii_case("tab") {
        Some(Scope::Tab)
    } else {
        None
    }
}

/// 把 egui 传来的修饰键状态与键名规范成一条和弦。
/// `command` 与 `ctrl` 任一为真都算 `Cmd`（跨平台）。
pub fn chord(command: bool, ctrl: bool, shift: bool, alt: bool, key: &'static str) -> Chord {
    Chord { cmd: command || ctrl, shift, alt, key }
}

/// 解析数据层里写死的和弦字符串。容错：多空格、大小写、写成 `Ctrl` / `Meta` 都接受。
///
/// **契约：只解析一条弦**。逗号在这里不是分隔符（`Cmd+Comma` 才是逗号键），
/// 所以 `"Alt+W, Alt+H"` 这种两段式文本在本函数里解析失败——那是
/// `parse_chord_seq` 的活。两者分开是有意的：单键与两段式键在派发上是两件事
/// （前者一次按下就够，后者要等第二次），语义不该混在一个返回值里。
pub fn parse_chord(text: &str) -> Option<Chord> {
    let mut cmd = false;
    let mut shift = false;
    let mut alt = false;
    let mut key: Option<&'static str> = None;
    for part in text.split('+') {
        let name = part.trim();
        // 纯空段（`Cmd++`）是分隔符残留，直接跳过；空白段才是空格键。
        if name.is_empty() && part.is_empty() {
            continue;
        }
        match name.to_ascii_lowercase().as_str() {
            // `meta` 是菜单快捷键列（`menu::shortcut_text`）的写法，与本层的
            // `Cmd` 是同一个键：mac 上 Cmd 就是 Meta，Win/Linux 上归一为 Ctrl。
            "cmd" | "ctrl" | "command" | "control" | "meta" => cmd = true,
            "shift" => shift = true,
            "alt" | "option" => alt = true,
            _ => {
                // 数据层是 &'static str 常量，这里用一张固定的键名表把
                // 任意拼写折回规范名，避免为一次比较分配 String。
                key = Some(canonical_key(part)?);
            }
        }
    }
    Some(Chord { cmd, shift, alt, key: key? })
}

/// 键名的规范形式表。`text()` 输出的一定是这里的某一项，
/// `parse_chord()` 也一定把它折回同一项，两者因此互补。
const LETTERS: &[&str] = &[
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M",
    "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
];

const DIGITS: &[&str] = &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

const FUNC_KEYS: &[&str] = &[
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14",
    "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24",
];

const NAMED_KEYS: &[&str] = &[
    "Enter", "Escape", "Tab", "Backspace", "Delete", "Insert", "Space", "Home", "End",
    "PageUp", "PageDown", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight",
    // 标点用 character 名，避免与 `+` 分隔符、空格、引号混淆。
    "Comma", "Period", "Slash", "Semicolon", "Quote", "OpenBracket", "CloseBracket",
    "Backslash", "Minus", "Equal", "Backquote", "Plus",
];

/// 别名 → 规范名。
const ALIASES: &[(&str, &str)] = &[
    ("return", "Enter"),
    ("esc", "Escape"),
    ("del", "Delete"),
    ("ins", "Insert"),
    ("pgup", "PageUp"),
    ("pgdn", "PageDown"),
    // 菜单快捷键列的写法是 `PgUp` / `PgDown`（视觉基线就是缩写）。数据层里
    // 有两条命令写着 `Meta+PgDown` / `Meta+PgUp`，它们必须先能解析，
    // 否则「菜单标了键、按下去不响」——`menu_check_tests.rs` 的第 3 / 4 条
    // 就是被这两个名字最先撞出来的。
    ("pgdown", "PageDown"),
    ("up", "ArrowUp"),
    ("down", "ArrowDown"),
    ("left", "ArrowLeft"),
    ("right", "ArrowRight"),
    ("bksp", "Backspace"),
    ("spacebar", "Space"),
];

/// 单字符写法 → 规范名；`+` 必须写 `Plus`，否则会被分隔符吃掉。
fn single_char_key(c: char) -> Option<&'static str> {
    Some(match c {
        ',' => "Comma",
        '.' => "Period",
        '/' => "Slash",
        ';' => "Semicolon",
        '\'' => "Quote",
        '[' => "OpenBracket",
        ']' => "CloseBracket",
        '\\' => "Backslash",
        '-' => "Minus",
        '=' => "Equal",
        '`' => "Backquote",
        ' ' => "Space",
        '+' => "Plus",
        _ => return None,
    })
}

fn canonical_key(s: &str) -> Option<&'static str> {
    let t = s.trim();
    if t.is_empty() {
        // 纯空白本身就是空格键（`parse_chord(" ")`）；空串则不是任何键。
        return if s.is_empty() { None } else { Some("Space") };
    }
    let mut chars = t.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if let Some(key) = single_char_key(c) {
            return Some(key);
        }
    }
    // 大小写不敏感地匹配规范名，再查别名表。
    LETTERS
        .iter()
        .chain(DIGITS)
        .chain(FUNC_KEYS)
        .chain(NAMED_KEYS)
        .copied()
        .find(|k| k.eq_ignore_ascii_case(t))
        .or_else(|| ALIASES.iter().find(|(a, _)| a.eq_ignore_ascii_case(t)).map(|(_, k)| *k))
}

/// 把数据层里的**键序**文本解析成一串和弦（`Alt+W, Alt+H` 是两段式）。
///
/// 分工：
/// - `parse_chord` 只认一条弦，逗号落在键名位置就整条解析失败；
/// - 本函数按 `,` 切段、逐段交给 `parse_chord`，**任何一段非法则整串非法**
///   （不做「能解析几段算几段」的容错：半条键序派发出去就是错的动作）。
///
/// 为什么不干脆让 `parse_chord` 吞逗号：单键与多段键的派发语义不同，
/// 返回值类型不同才分得开（`Chord` vs `Vec<Chord>`）。将来接真实派发时，
/// 前者的判定是「这一次按下」，后者是「一段有状态的序列」，不需要再改本层。
pub fn parse_chord_seq(text: &str) -> Option<Vec<Chord>> {
    text.split(',').map(parse_chord).collect()
}

/// 这条和弦是否命中表里的一项。
///
/// 单键按下的判定：绑定必须**只有一段**且等于按下的键。两段式绑定在这里
/// 恒为 `false`——按下第一段只是「进入等第二段的状态」，不是命中，
/// 那个状态由派发层维护（本层不存状态）。整段判定见 `matches_seq`。
pub fn matches(binding: &str, pressed: Chord) -> bool {
    matches_seq(binding, &[pressed])
}

/// 一串按键序列是否**整段**命中这条绑定。
///
/// 长度不等即不命中：绑定写了几段，就得按几段。
pub fn matches_seq(binding: &str, pressed: &[Chord]) -> bool {
    match parse_chord_seq(binding) {
        Some(want) => want.as_slice() == pressed,
        None => false,
    }
}

/// 两段式（或更长）键序列的等待上限，毫秒。
///
/// 1 000ms 取的是 vim `timeoutlen` 的出厂惯例，**不是**实测结论。
/// **超时不是正确性的必要条件**：`KeySeqState::feed` 的第 5 步
/// （既不命中整段、也不是前缀 → `Pass`）已经兜住所有「按错了」的情形，
/// 超时只解决一种边角——用户按了第一段就走开，很久以后又按了一个恰好是
/// 某条第二段的键，那一下会被吃成命中。
pub const KEYSEQ_TIMEOUT_MS: u64 = 1000;

/// 一次按键在键序列状态机里的一步结论。
///
/// 三个变体的区别只在一件事：**这一击要不要被吃掉**。
/// 吃掉意味着调用方不能再把它当单键绑定、也不能让终端看到它。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeqStep {
    /// 吃掉这一击：它是某条键序的前缀，状态机在等下一段。
    Pending,
    /// 吃掉这一击：整段命中，派发 `table[i]` 那条命令。
    Fire(usize),
    /// **不**吃这一击：既不是前缀，也让当前前缀作废。交回单键派发/终端。
    Pass,
}

/// 键序列状态机：只记「已按下的前缀」与「前缀开始的时刻」。
///
/// 它不读时钟（`now_ms` 由调用方注入，与 `trigger.rs` 的注入纪律同一个理由：
/// 注入才好测、才好回放），不认命令、不认 UI、不认识「哪个键归谁」——
/// 只回答「这一击算什么」。`table` 由调用方给：`(整段按键, 命令 ID)`。
///
/// 与 `matches` / `matches_seq` 的关系是**并列的两个入口**，不是替换：
/// - `matches` 回答「这一击单独看能不能命中单键绑定」，仍然对两段式绑定恒 false；
/// - 本状态机回答「这一击在一段等待中的序列里算什么」，是唯一能产出 `Fire`
///   的地方。把 `matches` 改成「长度 1 也算命中前缀」会让
///   `menu_check_tests.rs` 里「菜单写了键就必须按得响」那条判据变成恒真。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeySeqState {
    pending: Vec<Chord>,
    since_ms: u64,
}

impl KeySeqState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否正在等下一段（投影层用它决定要不要画提示、要不要吃掉事件）。
    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// 当前已按下的前缀，按按下顺序。
    pub fn pending(&self) -> &[Chord] {
        &self.pending
    }

    /// 丢掉前缀态。投影层在「进入重绑捕获态」时必须调一次（见 `dialogs::shortcuts`）。
    pub fn clear(&mut self) {
        self.pending.clear();
        self.since_ms = 0;
    }

    /// 喂一次按下，得到「这一击算什么」。
    ///
    /// 五步，**顺序是契约的一部分**：
    /// 1. 过期先清（`>` 而非 `>=`：正好等于上限的那一击仍然算数），
    ///    清完**继续**用空前缀判定这一击——它本身可能就是某条的第一段；
    /// 2. `candidate` = 已有前缀 + 这一击；
    /// 3. **整段命中先于前缀判定**：`Fire` 必须排在 `Pending` 前面，否则
    ///    `Meta+T, Meta+T` 这种「第二段同时是另一条的第一段」永远完不成；
    /// 4. 是某条的前缀 → 记下并 `Pending`；
    /// 5. 否则 `Pass`（前缀作废），调用方可以把它交给单键派发或终端。
    pub fn feed(
        &mut self,
        table: &[(Vec<Chord>, &'static str)],
        pressed: Chord,
        now_ms: u64,
    ) -> SeqStep {
        // 1. 过期先清，但**不返回**：这一击可能就是某条的第一段。
        if !self.pending.is_empty() && now_ms.saturating_sub(self.since_ms) > KEYSEQ_TIMEOUT_MS {
            self.clear();
        }
        // 2.
        let mut candidate = self.pending.clone();
        candidate.push(canonical_chord(pressed));
        // 3. 整段命中先于前缀判定。
        if let Some(i) = table.iter().position(|(seq, _)| seq.as_slice() == candidate) {
            self.clear();
            return SeqStep::Fire(i);
        }
        // 4. 还有更长的序列以它为前缀 → 等下一段。
        if table.iter().any(|(seq, _)| seq.starts_with(&candidate)) {
            self.pending = candidate;
            self.since_ms = now_ms;
            return SeqStep::Pending;
        }
        // 5. 既不命中也不是前缀：前缀作废，这一击不被吃掉。
        self.clear();
        SeqStep::Pass
    }
}

/// 一张绑定表里查第一个命中项，返回它的下标。
pub fn first_match(bindings: &[&str], pressed: Chord) -> Option<usize> {
    bindings.iter().position(|b| matches(b, pressed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    fn c(cmd: bool, shift: bool, alt: bool, key: &'static str) -> Chord {
        chord(cmd, false, shift, alt, key)
    }

    #[test]
    fn command_and_ctrl_both_normalize_to_cmd() {
        assert_eq!(chord(true, false, true, false, "P").text(), "Cmd+Shift+P");
        assert_eq!(chord(false, true, true, false, "P"), chord(true, false, true, false, "P"));
    }

    #[test]
    fn bindings_match_regardless_of_spacing_or_case() {
        let pressed = chord(true, false, true, false, "P");
        assert!(matches("Cmd + Shift + P", pressed));
        assert!(matches("ctrl+shift+p", pressed));
        assert!(!matches("Cmd+P", pressed));
    }

    #[test]
    fn chord_text_is_a_stable_display_form() {
        assert_eq!(parse_chord("cmd+shift+p").unwrap().text(), "Cmd+Shift+P");
        assert_eq!(parse_chord("Alt+F4").unwrap().text(), "Alt+F4");
    }

    #[test]
    fn first_match_reports_the_binding_index() {
        let table = ["Cmd+N", "Cmd+Shift+P", "Cmd+K"];
        assert_eq!(first_match(&table, chord(true, false, true, false, "P")), Some(1));
        assert_eq!(first_match(&table, chord(true, false, false, false, "K")), Some(2));
        assert_eq!(first_match(&table, chord(true, false, false, false, "Z")), None);
    }

    #[test]
    fn malformed_bindings_never_match_and_never_panic() {
        assert!(parse_chord("").is_none());
        assert!(!matches("Cmd+NotAKey", chord(true, false, false, false, "P")));
    }

    #[test]
    fn every_documented_key_name_round_trips() {
        for key in [
            "A", "Z", "0", "9", "F1", "F24", "Enter", "Escape", "Tab", "Backspace", "Delete",
            "Insert", "Space", "Home", "End", "PageUp", "PageDown", "ArrowUp", "ArrowDown",
            "ArrowLeft", "ArrowRight", "Comma", "Period", "Slash", "Semicolon", "Quote",
            "OpenBracket", "CloseBracket", "Backslash", "Minus", "Equal", "Backquote", "Plus",
        ] {
            let pressed = c(true, true, true, key);
            assert_eq!(parse_chord(&pressed.text()), Some(pressed), "{key}");
        }
    }

    #[test]
    fn aliases_and_symbols_resolve_to_canonical_names() {
        let key = |t: &str| parse_chord(t).map(|c| c.key);
        assert_eq!(key("esc"), Some("Escape"));
        assert_eq!(key("pgup"), Some("PageUp"));
        assert_eq!(key("del"), Some("Delete"));
        assert_eq!(key(" "), Some("Space"));
        assert_eq!(key(","), Some("Comma"));
        assert_eq!(key("`"), Some("Backquote"));
        assert_eq!(parse_chord("option+f4").unwrap().text(), "Alt+F4");
    }

    #[test]
    fn plus_is_a_key_name_not_a_separator() {
        assert_eq!(parse_chord("Cmd+Plus").unwrap().text(), "Cmd+Plus");
        assert_eq!(parse_chord("Plus").unwrap().key, "Plus");
        // 光写 `+` 或 `Cmd++` 只会被当成空段丢掉，绝不误认成 Plus。
        assert_eq!(parse_chord("+"), None);
        assert_eq!(parse_chord("Cmd++"), None);
    }

    /// 单条弦与键序是**两个契约**：`parse_chord` 只认一条，逗号归 `parse_chord_seq`。
    ///
    /// 这条钉住两者都不越界——`parse_chord` 不许吞逗号（否则单键绑定会静默
    /// 变成「半条键序」），`parse_chord_seq` 也不许多切（`Cmd+Comma` 仍是逗号键）。
    #[test]
    fn a_chord_carries_one_keypress_and_a_sequence_carries_several() {
        assert_eq!(parse_chord("Alt+W, Alt+H"), None, "单条弦不认逗号");
        let seq = parse_chord_seq("Alt+W, Alt+H").expect("两段式键位要能解析");
        assert_eq!(
            seq.iter().map(|c| c.text()).collect::<Vec<_>>(),
            ["Alt+W", "Alt+H"]
        );
        // 逗号键的写法不是 `,` 而是 `Comma`：切段之后仍然只有一条弦。
        assert_eq!(parse_chord_seq("Cmd+Comma").map(|s| s.len()), Some(1));
        assert_eq!(parse_chord_seq("Alt+;").map(|s| s.len()), Some(1));
        // 任何一段非法 -> 整串非法，不做「能解析几段算几段」。
        assert_eq!(parse_chord_seq("Alt+W, Alt+NotAKey"), None);
        assert_eq!(parse_chord_seq(""), None);
        assert_eq!(parse_chord_seq("Alt+W,"), None);
    }

    /// `matches` 是键序的「长度 1」特例，两段式键对单次按下恒不命中。
    ///
    /// 「按下第一段」不是命中，而是「正在等第二段」——那个状态由派发层维护，
    /// 本层不存状态，所以这里只能回答 false。
    #[test]
    fn matches_needs_the_whole_sequence_and_a_single_press_needs_one_chord() {
        let alt_w = parse_chord("Alt+W").unwrap();
        let alt_h = parse_chord("Alt+H").unwrap();
        assert!(!matches("Alt+W, Alt+H", alt_w), "单次按下不算命中两段式键");
        assert!(matches_seq("Alt+W, Alt+H", &[alt_w, alt_h]));
        assert!(!matches_seq("Alt+W, Alt+H", &[alt_w]));
        assert!(!matches_seq("Alt+W, Alt+H", &[alt_h, alt_w]), "顺序不能换");
        assert!(
            !matches_seq("Alt+W", &[alt_w, alt_h]),
            "单键绑定不吃多按一次"
        );
        assert!(matches("Alt+W", alt_w));
    }

    /// 菜单快捷键列（`menu::shortcut_text`）用的写法必须能解析回来。
    ///
    /// 两处都曾是「菜单标了键、按下去不响」的来源：`Meta+` 这个拼写和
    /// `PgDown` 这个缩写。数据层不改（它照视觉基线一字不差），改本层认它。
    #[test]
    fn the_menu_column_spellings_resolve_to_the_same_keys() {
        assert_eq!(parse_chord("Meta+N"), parse_chord("Cmd+N"));
        assert_eq!(parse_chord("Meta+PgDown").map(|c| c.key), Some("PageDown"));
        assert_eq!(parse_chord("Meta+PgUp").map(|c| c.key), Some("PageUp"));
        assert_eq!(parse_chord("Alt+'").map(|c| c.key), Some("Quote"));
        assert_eq!(parse_chord("Return"), parse_chord("Enter"));
        assert_eq!(parse_chord("Up"), parse_chord("ArrowUp"));
    }

    // ── 键序列状态机（两段式键的派发判据） ────────────────────────────────
    //
    // 表里的 ID 用假名：本文件是纯层，不许知道真命令叫什么。
    // 真实表在 `menu::DEFAULT_SHORTCUT_SEQS`，投影层构造时解析一次。

    /// 三段的测试表：一个共享前缀（`Alt+W`）带两个后继，一个两段同键（`Cmd+T, Cmd+K`）。
    fn seq_table() -> Vec<(Vec<Chord>, &'static str)> {
        vec![
            (parse_chord_seq("Alt+W, Alt+H").unwrap(), "split.h"),
            (parse_chord_seq("Alt+W, Alt+V").unwrap(), "split.v"),
            (parse_chord_seq("Cmd+T, Cmd+K").unwrap(), "one.keys"),
        ]
    }

    /// 状态机的一次喂入：`(结论, 是否还在等下一段)`。
    fn feed(state: &mut KeySeqState, text: &str, now_ms: u64) -> SeqStep {
        let pressed = parse_chord(text).expect("测试里的键必须能解析");
        state.feed(&seq_table(), pressed, now_ms)
    }

    /// 第一段被吃掉、进前缀态、不开火。
    #[test]
    fn a_prefix_press_is_consumed_without_firing() {
        let mut s = KeySeqState::new();
        assert_eq!(feed(&mut s, "Alt+W", 0), SeqStep::Pending);
        assert!(s.is_pending());
        assert_eq!(s.pending(), [parse_chord("Alt+W").unwrap()].as_slice());
    }

    /// 第二段接上：整段命中、前缀清空。
    #[test]
    fn the_second_segment_fires_the_whole_sequence_and_clears() {
        let mut s = KeySeqState::new();
        assert_eq!(feed(&mut s, "Alt+W", 0), SeqStep::Pending);
        assert_eq!(feed(&mut s, "Alt+H", 10), SeqStep::Fire(0));
        assert!(!s.is_pending(), "命中之后不能还留着前缀");
    }

    /// 第二段不认识：前缀作废，这一击交出去（不许被吃掉）。
    #[test]
    fn an_unmatched_second_key_clears_and_passes_through() {
        let mut s = KeySeqState::new();
        assert_eq!(feed(&mut s, "Alt+W", 0), SeqStep::Pending);
        assert_eq!(feed(&mut s, "Alt+Z", 10), SeqStep::Pass);
        assert!(!s.is_pending());
    }

    /// 超时：过期前缀不吃第二击；边界值（正好等于上限）仍然命中。
    ///
    /// 两条一起钉死比较符是 `>` 而不是 `>=`。
    #[test]
    fn a_stale_prefix_expires_and_the_next_press_passes() {
        let mut s = KeySeqState::new();
        assert_eq!(feed(&mut s, "Alt+W", 0), SeqStep::Pending);
        assert_eq!(
            feed(&mut s, "Alt+H", KEYSEQ_TIMEOUT_MS + 1),
            SeqStep::Pass,
            "过期之后第二段不许再命中——用户早走了"
        );
        assert!(!s.is_pending());

        // 边界：正好到上限仍然算数。
        let mut s = KeySeqState::new();
        assert_eq!(feed(&mut s, "Alt+W", 0), SeqStep::Pending);
        assert_eq!(feed(&mut s, "Alt+H", KEYSEQ_TIMEOUT_MS), SeqStep::Fire(0));
    }

    /// Esc 不是「取消前缀」的特例：它清前缀，但自己照常交出去。
    ///
    /// 这条防的是后人给 Esc 加一条「吃掉它」的分支——那会让终端里的 Esc
    /// 在最不该失灵的时候失灵（vim 用户会立刻发现）。
    #[test]
    fn escape_clears_a_pending_prefix_and_still_passes() {
        let mut s = KeySeqState::new();
        assert_eq!(feed(&mut s, "Alt+W", 0), SeqStep::Pending);
        assert_eq!(feed(&mut s, "Escape", 10), SeqStep::Pass);
        assert!(!s.is_pending());
    }

    /// 同一个前缀能通到两条不同的命令。
    #[test]
    fn the_same_prefix_can_lead_to_two_different_commands() {
        let mut h = KeySeqState::new();
        assert_eq!(feed(&mut h, "Alt+W", 0), SeqStep::Pending);
        assert_eq!(feed(&mut h, "Alt+H", 10), SeqStep::Fire(0));

        let mut v = KeySeqState::new();
        assert_eq!(feed(&mut v, "Alt+W", 0), SeqStep::Pending);
        assert_eq!(feed(&mut v, "Alt+V", 10), SeqStep::Fire(1));
    }

    /// 顺序有意义，与 `matches_seq` 一致。
    #[test]
    fn sequence_order_matters_like_matches_seq() {
        let mut s = KeySeqState::new();
        assert_eq!(feed(&mut s, "Alt+H", 0), SeqStep::Pass, "Alt+H 不是任何序列的第一段");
        assert_eq!(feed(&mut s, "Alt+W", 10), SeqStep::Pending);
        assert_eq!(feed(&mut s, "Alt+H", 20), SeqStep::Fire(0));
    }

    /// 第二段同时是单键绑定时：**整段命中优先**，空前缀时则让它落到单键。
    ///
    /// `Cmd+K` 在真实数据里就是这种情况（`Window.GotoLine` 的单键绑定，
    /// 同时是 `Meta+T, Meta+K` 的第二段）。这条守的是 `feed` 第 3 步
    /// 必须排在第 4 步之前。
    #[test]
    fn a_second_segment_that_is_also_a_single_binding_fires_the_sequence_first() {
        let mut s = KeySeqState::new();
        // 空前缀：Cmd+K 不是任何序列的整段，也不是任何序列的前缀 → 交回单键。
        assert_eq!(feed(&mut s, "Cmd+K", 0), SeqStep::Pass);
        // 有前缀：先判整段。
        assert_eq!(feed(&mut s, "Cmd+T", 10), SeqStep::Pending);
        assert_eq!(feed(&mut s, "Cmd+K", 20), SeqStep::Fire(2));
    }


    #[test]
    fn resolve_prefers_the_highest_scope() {
        let mut k = Keymap::new();
        let t = c(true, false, false, "T");
        k.bind(t, "global.tab", Scope::Global);
        k.bind(t, "window.tab", Scope::Window);
        assert_eq!(k.resolve(t), Some(("window.tab", Scope::Window)));
        k.bind(t, "tab.dup", Scope::Tab);
        assert_eq!(k.resolve(t), Some(("tab.dup", Scope::Tab)));
        assert_eq!(k.resolve(c(true, false, false, "Y")), None);
    }

    #[test]
    fn later_binding_wins_inside_one_scope_and_shows_up_as_conflict() {
        let mut k = Keymap::new();
        let t = c(true, false, false, "T");
        k.bind(t, "tab.default", Scope::Window);
        k.bind(t, "tab.user", Scope::Window);
        assert_eq!(k.resolve(t), Some(("tab.user", Scope::Window)));
        assert_eq!(k.conflicts(), vec![(t, "tab.default".to_string(), "tab.user".to_string())]);
        assert_eq!(k.len(), 2);
        assert!(!k.is_empty());
    }

    #[test]
    fn overlapping_scopes_are_priority_not_conflict() {
        let mut k = Keymap::new();
        let t = c(true, false, false, "T");
        k.bind(t, "a", Scope::Global);
        k.bind(t, "b", Scope::Tab);
        assert!(k.conflicts().is_empty());
    }

    #[test]
    fn unbind_removes_only_the_named_scope() {
        let mut k = Keymap::new();
        let t = c(true, false, false, "T");
        k.bind(t, "a", Scope::Global);
        k.bind(t, "b", Scope::Tab);
        assert!(k.unbind(t, Scope::Global));
        assert!(!k.unbind(t, Scope::Global));
        assert_eq!(k.resolve(t), Some(("b", Scope::Tab)));
        assert_eq!(k.bindings(Scope::Global).len(), 0);
    }

    #[test]
    fn bindings_are_sorted_and_stable() {
        let mut k = Keymap::new();
        k.bind(c(false, false, false, "B"), "b", Scope::Global);
        k.bind(c(false, false, true, "A"), "a", Scope::Global);
        k.bind(c(false, false, false, "Z"), "z", Scope::Tab);
        let texts: Vec<String> = k.bindings(Scope::Global).iter().map(|b| b.chord.text()).collect();
        assert_eq!(texts, vec!["Alt+A".to_string(), "B".to_string()]);
        let all: Vec<String> = k.bindings_all().iter().map(|b| b.chord.text()).collect();
        assert_eq!(all, vec!["Alt+A".to_string(), "B".to_string(), "Z".to_string()]);
        assert!(Keymap::new().is_empty());
        assert_eq!(Keymap::new().to_text(), "");
    }

    #[test]
    fn to_text_sorts_by_scope_then_chord_and_ends_with_newline() {
        let mut k = Keymap::new();
        k.bind(c(true, false, false, "T"), "tab.new", Scope::Tab);
        k.bind(c(true, true, false, "P"), "palette.open", Scope::Global);
        k.bind(c(true, false, true, "1"), "tab.goto1", Scope::Window);
        assert_eq!(
            k.to_text(),
            "Cmd+Shift+P = palette.open @global\nCmd+Alt+1 = tab.goto1 @window\nCmd+T = tab.new @tab\n"
        );
    }

    #[test]
    fn from_text_tolerates_junk_and_drops_bad_lines() {
        let text = "\n# 注释\n   \nCmd+Shift+P=palette.open @global\ncmd + t = tab.new@window\n\
                    Alt+1 = tab.goto1 @tab\nCmd+Q = quit @bogus\nCmd+NotAKey = nope @tab\n\
                    Cmd+W = @tab\nno equals sign here\n";
        let k = Keymap::from_text(text);
        assert_eq!(k.len(), 4, "只有四行是有效的");
        assert_eq!(k.resolve(c(true, true, false, "P")), Some(("palette.open", Scope::Global)));
        assert_eq!(k.resolve(c(true, false, false, "T")), Some(("tab.new", Scope::Window)));
        assert_eq!(k.resolve(c(false, false, true, "1")), Some(("tab.goto1", Scope::Tab)));
        // 未知作用域退化成 Global，而不是丢掉整行。
        assert_eq!(k.resolve(c(true, false, false, "Q")), Some(("quit", Scope::Global)));
    }

    #[test]
    fn text_round_trip_is_stable_including_duplicates() {
        let mut k = Keymap::new();
        k.bind(c(false, false, true, "1"), "tab.goto1", Scope::Tab);
        k.bind(c(true, false, false, "K"), "a.clear", Scope::Global);
        k.bind(c(true, false, false, "K"), "b.clear", Scope::Global); // 冲突也要往返
        k.bind(parse_chord("Cmd+Comma").unwrap(), "prefs.open", Scope::Global);
        k.bind(c(true, false, false, " "), "palette.toggle", Scope::Window);
        let back = Keymap::from_text(&k.to_text());
        assert_eq!(back.bindings_all(), k.bindings_all());
        assert_eq!(back.conflicts(), k.conflicts());
        assert_eq!(back.resolve(c(true, false, false, "K")), Some(("b.clear", Scope::Global)));
    }

    #[test]
    fn scope_ordering_matches_priority() {
        assert!(Scope::Global < Scope::Window);
        assert!(Scope::Window < Scope::Tab);
        assert_eq!(Scope::Global.cmp(&Scope::Window), Ordering::Less);
    }
}
