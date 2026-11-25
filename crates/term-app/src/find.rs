//! Find 能力：会话内查找的局部动作与局部 reducer。
//!
//! 之前 Find 的状态是「投影层直接写」的：`ui/find_bar.rs` 里的
//! `update_matches(&mut FindState, …)` 在绘制过程中把 `total_matches` /
//! `current_match` 写进 `ViewState`，等于投影层成了第二个写入者（违反单向依赖）。
//!
//! 把计算搬进了 reducer，但保留了「计数」这个缓存槽。
//! F2 发现缓存槽必然过期：查找的输入从「整块快照」变成「当前事实窗口」，
//! 而滚动会换窗口却不产生任何 `Action`，于是「3 / 7」会在回看后停在旧数字上。
//! 补法不是「滚动时也写一次」，而是**把计数从状态里删掉**——它是
//! `(query, 大小写, 窗口)` 的纯函数值，边界在 `matches()`，不缓存结果。
//! 2026-11-25：编译后的字面量正则按 `(query, 大小写)` 复用，属内部实现细节
//! ——匹配结果仍由窗口现算，滚动换窗口不会被旧计数/旧结果污染。
//!
//! 状态里因此只剩三件事：开没开、搜什么、区分大小写；外加一个
//! `current_match`（用户的「第几个」选择，按总数取模使用）。

use crate::state::{FindDirection, FindState};
use crate::vocab::Action;
use regex::Regex;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use term_core::find::{find_matches_regex_in, find_matches_regex_multiline_in, Match};
use term_core::grid::{glyph_columns, LineSlice, ScreenImage};

pub use crate::vocab::FindAction;

/// `SetQuery` 负载里保留的子命令前缀。
///
/// 不新增命令 ID，也不改 `FindAction` 词表：`≡` / `⇄▾` / Find All 这三个
/// 栏内开关是**动作参数**——用 `SetQuery` 这个已有入口携带一个不可能从
/// 键盘敲出来的 NUL 前缀，reducer 先识别它，识别不到才当普通查询串。
/// 这样命令表、快捷键表和菜单树一个字节都不用动。
const UI_CMD_PREFIX: &str = "\u{0}fb";

/// 上面那三个开关经 `SetQuery` 负载表达的子命令。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindUiCommand {
    CycleDirection,
    ToggleMultiLine,
    ToggleFindAll,
}

impl FindUiCommand {
    fn code(self) -> char {
        match self {
            Self::CycleDirection => 'd',
            Self::ToggleMultiLine => 'm',
            Self::ToggleFindAll => 'a',
        }
    }
}

/// 栏内开关的 FindAction（仍走 `SetQuery` 一个入口，参数不同）。
pub fn ui_command(cmd: FindUiCommand) -> FindAction {
    FindAction::SetQuery(format!("{UI_CMD_PREFIX}{}", cmd.code()))
}

/// 包成顶层 Action 供投影层 dispatch。
pub fn ui_command_action(cmd: FindUiCommand) -> Action {
    Action::Find(ui_command(cmd))
}

fn decode_ui_command(payload: &str) -> Option<FindUiCommand> {
    let rest = payload.strip_prefix(UI_CMD_PREFIX)?;
    match rest {
        "d" => Some(FindUiCommand::CycleDirection),
        "m" => Some(FindUiCommand::ToggleMultiLine),
        "a" => Some(FindUiCommand::ToggleFindAll),
        _ => None,
    }
}

/// 结果列表的一行：匹配序号 + 内容绝对行号 + 列区间 + 行摘要。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindResultRow {
    pub index: usize,
    pub row: i64,
    pub col_start: usize,
    pub col_end: usize,
    pub summary: String,
}

/// 结果列表的数据源：窗口 + 状态 → 行表（无缓存、无计数槽）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindResults {
    /// 取数时的查询串；UI 用它丢弃隔帧残留的列表。
    pub query: String,
    pub rows: Vec<FindResultRow>,
}

/// Find All 列表的数据源。复用 [`matches`] 的纯函数结果，不另立一套扫描。
pub fn result_rows(state: &FindState, window: &ScreenImage) -> FindResults {
    let query = state.query.clone();
    let found = matches(state, window);
    // 一次建好 row→行 索引：旧写法每条命中都扫一遍窗口行表，Find All 在大回看
    // 里命中越多越慢（O(行数×命中数)）。
    let line_index: HashMap<i64, &LineSlice> =
        window.lines.iter().map(|line| (line.row, line)).collect();
    let rows = found
        .iter()
        .enumerate()
        .filter_map(|(index, m)| {
            let summary = line_index.get(&m.row)?.text();
            Some(FindResultRow {
                index,
                row: m.row,
                col_start: m.col_start,
                col_end: m.col_end,
                summary,
            })
        })
        .collect();
    FindResults { query, rows }
}

/// `⇄▾` 决定 `<` / `>` / `Enter` 实际往哪边走。
///
/// `All` 不改变步进方向（仍是 `>` 向前），它的「范围」在打开结果列表上体现；
/// `Backward` 才真正把两个按钮的语义互换。抽成纯函数，栏里只读它派发。
pub fn nav_action(state: &FindState, forward: bool) -> FindAction {
    let backward = state.direction == FindDirection::Backward;
    if forward != backward {
        FindAction::Next
    } else {
        FindAction::Previous
    }
}

/// 点击结果列表某条时，给出「从当前匹配走到它」的最短动作与步数。
///
/// 不新增 GotoIndex 动作：现有 `Next` / `Previous` 在环形匹配集上走步，
/// 走 `min(前向, 后向)` 那一侧即可落在目标索引上。`total == 0` 时无动作。
pub fn jump_plan(state: &FindState, total: usize, target: usize) -> Option<(FindAction, usize)> {
    if total == 0 {
        return None;
    }
    let current = current_index(state, total)?;
    let target = target.min(total - 1);
    let forward = (target + total - current) % total;
    let backward = (current + total - target) % total;
    if forward <= backward {
        Some((FindAction::Next, forward))
    } else {
        Some((FindAction::Previous, backward))
    }
}


// ── 字面查找：编译结果按 (query, case_sensitive) 缓存 ──────────────
//
// `term_core::find::find_matches_in` 每次调用都 `regex::escape` + 编译一遍，
// 而查找栏每帧都问一次、常驻高亮每个词都问一次。编译结果只取决于
// `(query, case_sensitive)`，与窗口无关，所以在这一层缓存一次。缓存是内部实现
// 细节：对外仍是「参数 → 结果」的纯函数，全路径不读墙钟。

type LiteralCache = HashMap<(String, bool), Regex>;

static LITERAL_REGEX_CACHE: OnceLock<Mutex<LiteralCache>> = OnceLock::new();

/// 锁中毒时取回内部表继续用，不在生产路径新增 panic。
fn literal_cache_lock() -> MutexGuard<'static, LiteralCache> {
    let cache = LITERAL_REGEX_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 查询逐字增长会留下大量一次性模式；超过上限就整表丢弃，下次按需重建。
const LITERAL_CACHE_LIMIT: usize = 256;

fn literal_regex(query: &str, case_sensitive: bool) -> Option<Regex> {
    let key = (query.to_owned(), case_sensitive);
    let mut cache = literal_cache_lock();
    if let Some(re) = cache.get(&key) {
        return Some(re.clone()); // regex::Regex 内部是 Arc，克隆不重编译。
    }
    // 与 `term_core::find::find_matches_regex_in` 同一 builder 配置，匹配口径
    // （Unicode 简单大小写折叠）逐项一致。转义后的模式必定合法；意外失败只
    // 返回空结果，不退回另一种口径、也不 panic。
    let re = regex::RegexBuilder::new(&regex::escape(query))
        .case_insensitive(!case_sensitive)
        .build()
        .ok()?;
    if cache.len() >= LITERAL_CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(key, re.clone());
    #[cfg(test)]
    note_literal_compile(query, case_sensitive);
    Some(re)
}

// 编译次数按 (query, case) 记在测试专用表里：测试并发跑时不依赖全局缓存长度，
// 只认自己那个唯一查询串的编译次数。
#[cfg(test)]
static LITERAL_COMPILES: OnceLock<Mutex<HashMap<(String, bool), usize>>> = OnceLock::new();

#[cfg(test)]
fn note_literal_compile(query: &str, case_sensitive: bool) {
    let cache = LITERAL_COMPILES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *cache.entry((query.to_owned(), case_sensitive)).or_insert(0) += 1;
}

#[cfg(test)]
fn literal_compiles_for(query: &str, case_sensitive: bool) -> usize {
    let cache = LITERAL_COMPILES.get_or_init(|| Mutex::new(HashMap::new()));
    let cache = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.get(&(query.to_owned(), case_sensitive)).copied().unwrap_or(0)
}

/// 预建 row → 字形表。整词过滤与结果列表都按 row 取行，避免对每条命中
/// 线性搜索窗口行表（大回看 × 多命中 = O(行数×命中数)）。
fn row_glyphs(window: &ScreenImage) -> HashMap<i64, (Vec<usize>, Vec<char>)> {
    window
        .lines
        .iter()
        .map(|line| {
            let cols = glyph_columns(&line.cells);
            let chars = cols.iter().map(|&c| line.cells[c].ch).collect();
            (line.row, (cols, chars))
        })
        .collect()
}

/// 字面查找入口：与 `term_core::find::find_matches_in` 同语义（同一转义、
/// 同一大小写折叠、同一列坐标、同一整词边界），但复用编译好的正则。
/// `pane_overlays` 的常驻高亮也走这里，每个词不再各编译一次。
pub fn find_matches_literal_cached(
    window: &ScreenImage,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
) -> Vec<Match> {
    if query.is_empty() || window.cols == 0 {
        return Vec::new();
    }
    let Some(re) = literal_regex(query, case_sensitive) else {
        return Vec::new();
    };
    let mut matches = Vec::new();
    for line in &window.lines {
        let cols = glyph_columns(&line.cells);
        if cols.is_empty() {
            continue;
        }
        let text: String = cols.iter().map(|&c| line.cells[c].ch).collect();
        // regex 给的是字节偏移，折叠表按字形索引：每个字节映射到包含它的字形。
        let mut byte_to_char = vec![0usize; text.len() + 1];
        for (idx, (start, ch)) in text.char_indices().enumerate() {
            for b in start..start + ch.len_utf8() {
                byte_to_char[b] = idx;
            }
        }
        for m in re.find_iter(&text) {
            if m.start() == m.end() {
                continue; // 空匹配不画高亮。
            }
            let first = byte_to_char[m.start()];
            let last = byte_to_char[m.end() - 1];
            let col_start = cols[first];
            let col_end = cols[last] + if line.cells[cols[last]].wide { 2 } else { 1 };
            matches.push(Match::new(line.row, col_start, col_end));
        }
    }
    if whole_word {
        let index = row_glyphs(window);
        matches.retain(|m| {
            let Some((cols, chars)) = index.get(&m.row) else {
                return false;
            };
            let Some(at) = cols.iter().position(|&c| c == m.col_start) else {
                return false;
            };
            let len = cols[at..].iter().take_while(|&&c| c < m.col_end).count();
            len > 0 && on_word_boundaries(chars, at, len)
        });
    }
    matches
}

fn on_word_boundaries(chars: &[char], at: usize, len: usize) -> bool {
    let before_ok = at == 0 || !is_word_char(chars[at - 1]);
    let after_ok = at + len >= chars.len() || !is_word_char(chars[at + len]);
    before_ok && after_ok
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// 窗口内的全部匹配：`(query, 大小写, 窗口)` 的纯函数值。
///
/// 「窗口」是用户此刻看得见的那一屏（含回看部分），所以贴在底部时
/// 就是当前屏，滚进历史时就是历史——同一份代码，不需要分支。
///
/// 栏没打开时恒为空：隐藏状态下谁也别想拿到计数，
/// 否则「不可见但仍在统计」迟早会被人当成状态用。
pub fn matches(state: &FindState, window: &ScreenImage) -> Vec<Match> {
    if !state.visible || state.query.is_empty() {
        return Vec::new();
    }
    if state.regex {
        // 表达式写错时返回空集合（显示 0/N），而不是退回字面匹配——那会让
        // 用户以为 `[` 真的搜到了东西。UI 另有语法校验提示（见 find_bar）。
        // `≡` 在正则模式下放开 `.` 跨行；纯层支持，不是画一个假开关。
        if state.multi_line {
            find_matches_regex_multiline_in(window, &state.query, state.case_sensitive)
                .unwrap_or_default()
        } else {
            find_matches_regex_in(window, &state.query, state.case_sensitive).unwrap_or_default()
        }
    } else {
        find_matches_literal_cached(window, &state.query, state.case_sensitive, state.whole_word)
    }
}

/// 当前匹配在匹配集合里的下标；没有匹配时为 `None`。
///
/// 对总数取模：窗口一变总数就变，取模保证 `current_match` 永远不会指到
/// 集合外面，也不需要在换窗口时「帮忙修正」它。
pub fn current_index(state: &FindState, total: usize) -> Option<usize> {
    if total == 0 {
        None
    } else {
        Some(state.current_match % total)
    }
}

/// 纯函数：只改 `FindState`，匹配集合由「窗口 + query」确定性推导。
///
/// `Next` / `Previous` 在匹配集合上循环，而不是从光标位置重新搜索——
/// 前者与「N / M」计数天然一致，也不会因为光标停在匹配内部而原地打转。
/// 命令 ID → 本能力的动作。未接线的 ID 返回 `None`。
///
/// 2026-11-12：实现只有一份，在 [`crate::parse`]（入口层）。这里保留同名入口，
/// 一是因为「每个能力自带字符串入口」是这个 crate 对外的约定，二是因为
/// `apps/desktop` 有直接调 `term_app::log::parse` 的地方——把路径改掉没有架构价值。
pub fn parse(id: &str) -> Option<FindAction> {
    crate::parse::find_parse(id)
}

pub fn reduce(state: &mut FindState, action: FindAction, window: Option<&ScreenImage>) {
    match action {
        FindAction::Open => {
            state.visible = true;
            state.query.clear();
            state.case_sensitive = false;
            state.whole_word = false;
            state.regex = false;
            state.multi_line = false;
            state.direction = FindDirection::Forward;
            state.find_all_open = false;
            state.current_match = 0;
        }
        FindAction::SetQuery(q) => match decode_ui_command(&q) {
            // 三个栏内开关走「动作参数」：不换查询、不动当前匹配，只翻自己的位。
            Some(FindUiCommand::CycleDirection) => {
                state.direction = match state.direction {
                    FindDirection::Forward => FindDirection::Backward,
                    FindDirection::Backward => FindDirection::All,
                    FindDirection::All => FindDirection::Forward,
                };
                // `All` 是「范围 = 整个窗口」，展开结果列表才是它的可见效果。
                state.find_all_open = state.direction == FindDirection::All;
            }
            Some(FindUiCommand::ToggleMultiLine) => {
                state.multi_line = !state.multi_line;
            }
            Some(FindUiCommand::ToggleFindAll) => {
                state.find_all_open = !state.find_all_open;
            }
            None => {
                state.query = q;
                // 换了查询就回到第一个匹配——否则「第 9 个」这种残留索引
                // 会让人以为新查询是从中间开始搜的。
                state.current_match = 0;
            }
        },
        FindAction::ToggleCase => {
            state.case_sensitive = !state.case_sensitive;
            state.current_match = 0;
        }
        // 换匹配口径 = 换了一个匹配集合，与改大小写同理要回到第一个。
        FindAction::ToggleWholeWord => {
            state.whole_word = !state.whole_word;
            state.current_match = 0;
        }
        // 正则与整词同理：换匹配口径就换了一个匹配集合，回到第一个。
        FindAction::ToggleRegex => {
            state.regex = !state.regex;
            state.current_match = 0;
        }
        // 栏关着时按 Find Next，之前只挪了 `current_match`：没人看得见
        // （绘制条件是 `find.visible`），而且 `Open` 会把 `current_match` 归零，
        // 等于什么都没留下。兄弟动作 `SelectionNext`/`SelectionPrev` 早就先开栏，
        // 这里对齐它们；但**不清 `query`**——「在现有查询上再走一步」是 Next 的语义，
        // 清了就变成「重新查一次空串」。
        FindAction::Next => {
            state.visible = true;
            step(state, window, 1)
        }
        // 同上：开栏 + 保留查询，只是在匹配集合上反向走一步。
        FindAction::Previous => {
            state.visible = true;
            step(state, window, -1)
        }
        FindAction::Close => {
            state.visible = false;
            state.query.clear();
            state.current_match = 0;
            state.find_all_open = false;
        }
        // 以选区为查询词的前置复位——打开栏、清空旧查询、回到第一个匹配。
        // 「填选区 + 跳一步」交给投影层（`SelectionNext`/`SelectionPrev` 在 action.rs
        // 路由成 `Effect::FindSelection(±1)`）。
        FindAction::SelectionNext | FindAction::SelectionPrev => {
            state.visible = true;
            state.query.clear();
            state.current_match = 0;
        }
    }
}

fn step(state: &mut FindState, window: Option<&ScreenImage>, delta: i64) {
    let total = window.map(|w| matches(state, w).len()).unwrap_or(0);
    state.current_match = match current_index(state, total) {
        Some(cur) => (cur as i64 + delta).rem_euclid(total as i64) as usize,
        None => 0,
    };
}

// 单测独立成文件：`find.rs` 的代码行不能再往上涨（4j 上限 500）。
#[cfg(test)]
#[path = "find_tests.rs"]
mod tests;
