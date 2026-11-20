//! hex 视图的**状态与纯函数**：原始字节环、视口锚点、一行怎么排出来。
//!
//! 与 `bin.rs` 分开的理由只有一个（R9 的行数上限）：本文件**零 `egui`**，
//! 环与排版都是「进字节、出字符串」的纯逻辑，因此可以直接单测，不必为了
//! 测一次「第 17 个字节有没有换行」去造一个窗口。分法照
//! `xfer_spec.rs` / `tools_sender_state.rs` 的先例。
//!
//! # 为什么是「一直存在的原始字节流」，不是「Bin 模式的缓存」
//!
//! 第一版设计让 tap 只在切到 Bin 时才挂、`enter()` 时清环。那等于
//! **每次切过去屏幕都是空的**，要等新数据才有内容——而用户点 Hex Edit
//! 想看的恰恰是**刚才那一段**。所以：
//!
//! 1. 每个标签从**创建那一刻**就挂 tap（`AttachTap` 是非破坏性 tee，
//!    VT 照吃，屏幕不受影响），环的寿命 = 标签的寿命；
//! 2. 环里存什么**与屏幕上的模式无关**，切模式只是换渲染器；
//! 3. 只有标签关闭才 detach 并释放环。
//!
//! 这一条是本模块所有设计判断的源头：**凡是想「切模式时清一下环」的改动，
//! 都是在把「换一种看法」退化成「丢一次历史」**。
//!
//! 内存不是问题：`VecDeque` 按需生长，没收到字节的标签几乎不占内存
//! （`RING_CAP` 只是封顶，不是预分配）。
//!
//! # 排版参数不在这里
//!
//! 三张档位表、`HexParams`、分隔符规则、窗宽夹取都在 `hex_params.rs`：
//! 本文件只回答「有哪些字节可画」，那边回答「一行长什么样」。`row_text`
//! 留在这里是因为它按环里的字节切行（贴着 [`BinBuffer::take`]），
//! 而它的宽度与分隔符全部来自 `hex_params`——排版仍只有一份算法。

use std::collections::VecDeque;

use term_core::coords::ByteOffset;

use crate::hex_tap::HexTap;
use super::hex_params::{byte_text, full_row_chars, sep_after, HexParams, OFFSET_DIGITS};

/// 每个标签最多留多少原始字节（1 MiB）。
///
/// 封顶而不是无限：`hexdump` 想看的永远是**最近**那一段，而 1 MiB 在 16 列下
/// 是 65536 行，翻到手酸也翻不完。超出的从头丢、`base` 前进（见 [`BinBuffer`]）。
pub(in crate::ui) const RING_CAP: usize = 1 << 20;

/// 一行：偏移（8 位大写 hex）+ 两个空格 + 按 `params` 排版的字节。
///
/// 分隔符规则**不在本文件**：它只有一份实现，在
/// `hex_params::sep_after`（`row_text` / `full_row_chars` / `byte_char_at`
/// 三处共用，改一处三处一起动）。这里只负责把字节与分隔符拼起来。
///
/// 规则本身（一句话，嵌套的两条判据）：写完第 `k` 个字节之后，
/// `k` 不是 `group` 的整倍数就**一格都不加**（组内连写，加宽也不例外）；
/// 是 `group` 的整倍数时，再看它是不是 `division` 的整倍数——是就加两格，
/// 不是就加一格。行尾一格都不加。
/// 所以 `group = 8 / division = 4` 是「八个字节连写、第 8 个后面两格」，
/// 而不是「每 4 个字节加宽一次」：组还没结束，谈不上段与段之间。
///
/// 尾行不足 `columns` 字节时**不补占位符**：`--` 或 `··` 会让「这一行只有 5 个
/// 字节」看起来像「这一行有 16 个字节，后 11 个是特殊值」。短就是短。
pub(in crate::ui) fn row_text(bytes: &[u8], offset: u64, params: HexParams) -> String {
    let mut out = String::with_capacity(full_row_chars(params));
    out.push_str(&format!("{offset:0OFFSET_DIGITS$X}"));
    out.push_str("  ");
    let count = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        out.push_str(&byte_text(*b, params.base));
        let k = i + 1;
        if k == count {
            continue;
        }
        match sep_after(k, params) {
            2 => out.push_str("  "),
            1 => out.push(' '),
            _ => {}
        }
    }
    out
}

/// 一个标签的**原始字节环**：只追加、超上限从头丢。
///
/// 三个数各管一件事，别合并：
/// - `base`：环里第一个字节的**绝对偏移**（丢了 N 个字节它就是 N）；
/// - `total`：从开始到现在**一共**收到多少字节（只增）；
/// - `len()`：环里现在还剩几个。
///
/// `dropped()` 正好等于 `base`——两个来源留一个就够，但两条都要能查，
/// 因为界面上一处说「丢了多少」、另一处要用绝对偏移。
pub(in crate::ui) struct BinBuffer {
    bytes: VecDeque<u8>,
    base: u64,
    total: u64,
    cap: usize,
}

impl BinBuffer {
    pub(in crate::ui) fn new() -> Self {
        Self::with_cap(RING_CAP)
    }

    pub(in crate::ui) fn with_cap(cap: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            base: 0,
            total: 0,
            cap: cap.max(1),
        }
    }

    /// 追加一块字节。超出上限的部分**从头丢**，`base` 跟着前进。
    pub(in crate::ui) fn push(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.total += data.len() as u64;
        self.bytes.extend(data.iter().copied());
        let over = self.bytes.len().saturating_sub(self.cap);
        if over > 0 {
            // 一次性 drain 而不是 pop_front 循环：1 MiB 上限下这是每块最多一次
            // 线性搬移，循环则是每字节一次。
            self.bytes.drain(..over);
            self.base += over as u64;
        }
    }

    /// 环里第一个字节的绝对偏移。
    pub(in crate::ui) fn first(&self) -> u64 {
        self.base
    }

    /// 环尾（下一个字节会落在这里）。也是「已收到的最后位置」。
    pub(in crate::ui) fn end(&self) -> u64 {
        self.base + self.bytes.len() as u64
    }

    /// 累计收到多少字节（含已被丢掉的）。
    pub(in crate::ui) fn total(&self) -> u64 {
        self.total
    }

    /// 环里现在有多少字节。
    pub(in crate::ui) fn len(&self) -> usize {
        self.bytes.len()
    }

    /// 已经丢掉了多少字节。
    pub(in crate::ui) fn dropped(&self) -> u64 {
        self.total - self.bytes.len() as u64
    }

    /// 取 `[from, from+len)`，越界部分**收敛**而不是 panic。
    ///
    /// 收敛是必须的：`anchor` 记的是上一帧的偏移，这一帧环可能已经把它丢了
    /// （洪水输出时 base 每轮都在前进）。渲染路径上任何一次 panic 都是整窗崩。
    pub(in crate::ui) fn take(&self, from: u64, len: usize) -> Vec<u8> {
        let start = from.max(self.base).min(self.end());
        let stop = start.saturating_add(len as u64).min(self.end());
        let i = (start - self.base) as usize;
        let j = (stop - self.base) as usize;
        self.bytes.iter().skip(i).take(j - i).copied().collect()
    }
}

impl Default for BinBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// 一个标签的原始字节流：环 + tap 接收端 + 视口锚点。
///
/// 名字里没有「bin」是**故意的**：它不是「Bin 模式的缓存」，是这个标签
/// 从生到死一直在收的原始字节流。叫 `bin` 会让下一个人以为「不在 Bin 模式
/// 就可以不清它 / 可以只在切过去时才挂 tap」——那正是第一版设计的错。
pub(in crate::ui) struct RawLog {
    pub ring: BinBuffer,
    /// 监管者线程 tee 过来的字节。`None` = 还没挂上（或已摘）。
    ///
    /// 类型是 `HexTap`（`apps/desktop/src/hex_tap.rs`）而不是裸的 `Receiver`：
    /// B2 不许 `ui/` 持有 OS 资源，句柄在这里、资源在那层。
    pub tap: Option<HexTap>,
    /// 视口顶部那一行的**绝对字节偏移**；`None` = 贴底跟随。
    ///
    /// 用字节偏移而不是行号：换 `columns` / 换字号之后「第几行」指向的内容
    /// 会变，而「第几个字节」不变。用户回看时滚到的是内容，不是行号。
    ///
    /// 类型是 [`ByteOffset`] 而不是裸 `u64`：环首偏移、环尾偏移、视口顶部在这
    /// 里都长得像整数，谁减谁只能靠注释记。给了类型之后，「这是一个字节偏移」
    /// 在签名上就成立了；换算仍走 `.get()` 出来的裸整数——`coords` 刻意只给
    /// `checked_add`、不给 `Add`/`Sub`，本文件也就不给自己发明算术。
    pub anchor: Option<ByteOffset>,
}

impl RawLog {
    pub(in crate::ui) fn new() -> Self {
        Self {
            ring: BinBuffer::new(),
            tap: None,
            anchor: None,
        }
    }

    /// 挂上 tap。覆盖式：同一条会话重复挂只会保留最后一个接收端。
    pub(in crate::ui) fn attach(&mut self, tap: HexTap) {
        self.tap = Some(tap);
    }

    /// 摘掉 tap（标签关闭 / 会话断开）。环**不清**：断开之后用户还该能看
    /// 断开前收到的那一段，清掉等于「一断开就什么都不剩」。
    pub(in crate::ui) fn detach(&mut self) {
        self.tap = None;
    }

    /// 把 tap 里已经到达的字节全部搬进环，返回搬了多少。
    ///
    /// **每帧都要调，Text 模式也要**：tap 是无界 channel，不搬就会一直攒在
    /// 通道里；而切到 Bin 之前如果没搬过，历史就少了那一段——用户看到的第一屏
    /// 会从「切过去的那一刻」开始，正是本模块要消灭的那个 bug。
    /// 调用点是 `Tab::poll`（**只那一处**，见那里的注释）。
    pub(in crate::ui) fn drain(&mut self) -> usize {
        // 解构取两个不相交的字段借用：`tap` 是共享、`ring` 是可变。
        // 不这么写就只有两条路——把 `tap` 先 `take()` 出来再放回（中间失败会
        // 丢句柄），或者给 `HexTap::drain` 传一个 `&mut BinBuffer`（那会让
        // app 层反过来认识 ui 的类型）。两条都不如解构。
        let Self { ring, tap, .. } = self;
        let Some(tap) = tap.as_ref() else {
            return 0;
        };
        tap.drain(|chunk| ring.push(chunk))
    }

    /// 把 tap 里已到达的字节搬进环，并把本帧的原始字节**收成 owned 值交回去**。
    ///
    /// 与 `drain` 只差最后一步：B2 的 tmux 控制行要复用同一份字节，但
    /// `&mut self.raw` 与 `&mut self.tmux` 不能同时塞进一个闭包。字节在
    /// `HexTap::drain_owned`（app 层）收成一份 `Vec<u8>` 再交回来，借用边界
    /// 就不跨 `ui/` 的两个字段；这里只做「推一次环 + 原样交回」，把 R9 余量
    /// 留给排版逻辑。空 tap 时返回 `Vec::new()`（零分配），调用方据此跳过消费。
    /// 环是平坦字节流（`BinBuffer::push` 逐字节 append），一次推与逐块推等价。
    pub(in crate::ui) fn drain_take(&mut self) -> Vec<u8> {
        let Self { ring, tap, .. } = self;
        let Some(tap) = tap.as_ref() else {
            return Vec::new();
        };
        let taken = tap.drain_owned();
        ring.push(&taken);
        taken
    }

    /// 回到贴底跟随（切到 Bin 时调一次：新进来的人想看最新）。
    pub(in crate::ui) fn follow(&mut self) {
        self.anchor = None;
    }

    /// 贴底时视口顶部应落在哪个字节。环还装不满一屏时就落在环首。
    pub(in crate::ui) fn tail_anchor(&self, view_rows: usize, params: HexParams) -> ByteOffset {
        let span = view_rows as u64 * params.columns.max(1) as u64;
        ByteOffset::new(self.ring.end().saturating_sub(span).max(self.ring.first()))
    }

    /// 滚动 `rows` 行（正 = 往更早的字节看，负 = 往更新的看）。
    ///
    /// 滚到底自动回到 `anchor = None`（贴底跟随），而不是停在一个「正好等于
    /// 尾部」的固定偏移上：后者在新数据到达时不会跟着走，用户会以为输出停了。
    pub(in crate::ui) fn scroll_rows(&mut self, rows: i64, view_rows: usize, params: HexParams) {
        if rows == 0 {
            return;
        }
        let step = params.columns.max(1) as i64;
        // 进 i64 之前先 `.get()`：`ByteOffset` 只保证「是个字节偏移」，回看算式
        // 是行数的加减法，两者在这一行交界一次，之后全是裸整数。
        let tail = self.tail_anchor(view_rows, params).get();
        let top = self.anchor.map_or(tail, ByteOffset::get);
        let want = top as i64 - rows.saturating_mul(step);
        let lo = self.ring.first().min(tail) as i64;
        let hi = tail.max(self.ring.first()) as i64;
        let clamped = want.clamp(lo, hi);
        self.anchor = if clamped >= hi {
            None
        } else {
            Some(ByteOffset::new(clamped.max(0) as u64))
        };
    }

    /// 本帧要画的那些行：`(该行第一个字节的绝对偏移, 这一行的字节)`。
    ///
    /// 行从**环首**开始切，不按 16 的倍数对齐：环首偏移是任意的（洪水下每次
    /// 丢的字节数不一定是整行），硬对齐会让第一行「声称有 16 个字节、实际
    /// 只有后半截」——那是在屏幕上编造不存在的字节。
    pub(in crate::ui) fn visible(
        &self,
        view_rows: usize,
        params: HexParams,
    ) -> Vec<(u64, Vec<u8>)> {
        let cols = params.columns.max(1) as u64;
        let end = self.ring.end();
        let mut off = self
            .anchor
            .map_or_else(|| self.tail_anchor(view_rows, params).get(), ByteOffset::get)
            .max(self.ring.first());
        let mut out = Vec::with_capacity(view_rows);
        while off < end && out.len() < view_rows {
            let n = (end - off).min(cols) as usize;
            out.push((off, self.ring.take(off, n)));
            off += cols;
        }
        out
    }

    /// 已经丢掉的字节数（界面上「N bytes dropped」用它）。
    pub(in crate::ui) fn dropped(&self) -> u64 {
        self.ring.dropped()
    }
}

impl Default for RawLog {
    fn default() -> Self {
        Self::new()
    }
}

/// 本帧可见的 hex 文本：一行一段、**每行以 `\n` 结尾**。
///
/// 给 `Cmd+C` 用（复制看得见那一屏）。行尾补 `\n` 是为了粘进编辑器时
/// 一行就是一行，不必让每个消费者自己再拼。
pub(in crate::ui) fn visible_hex_text(
    log: &RawLog,
    view_rows: usize,
    params: HexParams,
) -> String {
    let mut out = String::new();
    for (offset, bytes) in log.visible(view_rows, params) {
        out.push_str(&row_text(&bytes, offset, params));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::hex_params::{needed_width, DIVISION, HEX_BASES, HEX_COLUMNS_CHOICES, HEX_GROUP_CHOICES};
    use super::*;
    use crate::hex_tap::HexTap;
    use std::sync::mpsc::channel;

    fn p(base: u32, columns: usize, group: usize) -> HexParams {
        HexParams {
            base,
            columns,
            group,
            division: DIVISION,
            // 排版夹具不画底色：斑马线在这几条测的事（环 / 行文本）里没有读者，
            // 但要填上——`HexParams` 是一个整体，缺一项就少一次「五个参数都被
            // 想到」的机会。
            zebra: true,
        }
    }

    // ---- 环 ----

    #[test]
    fn the_ring_keeps_the_tail_and_counts_everything() {
        let cap = 1 << 20;
        let mut ring = BinBuffer::with_cap(cap);
        // 2 MiB 分两块灌进去：一次 1.5 MiB、一次 0.5 MiB。
        ring.push(&vec![0x41; cap + cap / 2]);
        ring.push(&vec![0x42; cap / 2]);

        assert_eq!(ring.len(), cap, "环不该超过上限");
        assert_eq!(ring.total(), 2 * cap as u64, "total 只增，等于全部收到的");
        assert_eq!(ring.first(), cap as u64, "丢掉了整整 1 MiB，base 跟着前进");
        assert_eq!(ring.end(), 2 * cap as u64);
        assert_eq!(ring.dropped(), cap as u64);

        let head = ring.take(ring.first(), 16);
        assert_eq!(head.len(), 16, "取环首 16 字节");
        assert!(head.iter().all(|b| *b == 0x41), "环首还是第一块的内容");

        // 越界一律收敛，不 panic：洪水下 anchor 每帧都可能过期。
        // 起点早于环首 → 收敛到环首（`visible()` 调用前已经夹过一次，这里是兜底）。
        assert_eq!(ring.take(0, 16).len(), 16, "早于环首收敛到环首");
        assert_eq!(ring.take(ring.end() - 4, 16).len(), 4, "尾部只有 4 字节");
        assert!(ring.take(ring.end(), 16).is_empty(), "从环尾起取 → 空");
        assert!(ring.take(u64::MAX, 16).is_empty(), "越到天上去也不 panic");
    }

    #[test]
    fn a_ring_never_reports_more_than_it_was_given() {
        let mut ring = BinBuffer::with_cap(4);
        ring.push(b"ab");
        assert_eq!((ring.first(), ring.end(), ring.total()), (0, 2, 2));
        ring.push(b"cdef"); // 6 字节总量，cap 4 → 从头丢 2
        assert_eq!(ring.len(), 4);
        assert_eq!(ring.first(), 2);
        assert_eq!(ring.take(0, 4), b"cdef".to_vec(), "留下的是后 4 个字节");
        assert_eq!(ring.dropped(), 2);
    }

    /// 洪水夹具必须真的把环打满：否则基线里不会出现「N bytes dropped」，
    /// 那张图就白拍了。
    ///
    /// `BIN_OVERFLOW_BYTES` 定义在 `demo.rs`——那里读不到 `RING_CAP`
    /// （`bin_spec` 对 crate 其余模块不可见），所以这条关系只能在这里钉。
    #[test]
    fn the_overflow_fixture_really_overflows_the_ring() {
        let mut ring = BinBuffer::new();
        ring.push(&vec![0u8; crate::demo::BIN_OVERFLOW_BYTES]);
        assert_eq!(ring.len(), RING_CAP, "留下来的正好是一整个环");
        assert!(ring.dropped() > 0, "洪水夹具没触发丢弃，基线就少了一条判据");
        assert!(ring.first() > 0, "丢弃之后偏移列不从 0 开始");
    }

    // ---- 排版 ----

    /// 默认参数（16 进制 / 16 列 / 每 1 字节一组）下的**满行**。
    ///
    /// 这条是**精确字符串**断言：多一个空格、偏移少一位、大小写变了都红。
    /// `group = 1` 就是经典 `hexdump`——字节之间一格，每逢第 4 / 8 / 12 个字节
    /// 加宽成两格（`DIVISION`），所以能一眼按 4 字节数位置。
    #[test]
    fn a_full_row_is_the_classic_hexdump() {
        let bytes: Vec<u8> = (0x41..=0x50u8).collect(); // 'A'..='P'
        assert_eq!(
            row_text(&bytes, 0, HexParams::default()),
            "00000000  41 42 43 44  45 46 47 48  49 4A 4B 4C  4D 4E 4F 50"
        );
    }

    /// 短行：同一条分隔符规则，只是末尾不留空格。
    ///
    /// 5 个字节正好跨过一次加宽边界（第 4 个字节之后），所以 `44` 与 `45`
    /// 之间是两个空格、`45` 后面什么都不加。
    #[test]
    fn a_row_is_rendered_exactly_this_way() {
        let want = "00000000  41 42 43 44  45";
        assert_eq!(row_text(&[0x41, 0x42, 0x43, 0x44, 0x45], 0, HexParams::default()), want);
    }

    /// 尾行不补占位符：短就是短，别在屏幕上编造字节。
    #[test]
    fn a_short_tail_row_is_not_padded() {
        let text = row_text(&[0xff], 0x1234, HexParams::default());
        assert_eq!(text, "00001234  FF");
        assert!(!text.contains("--"));
    }

    /// 五种进制各自的位宽：2 → 8 位、7 / 8 / 10 → 3 位、16 → 2 位。
    #[test]
    fn every_base_keeps_its_own_digit_width() {
        let b = 0x41u8; // 65
        assert_eq!(row_text(&[b], 0, p(2, 16, 1)), "00000000  01000001");
        assert_eq!(row_text(&[b], 0, p(7, 16, 1)), "00000000  122");
        assert_eq!(row_text(&[b], 0, p(8, 16, 1)), "00000000  101");
        assert_eq!(row_text(&[b], 0, p(10, 16, 1)), "00000000  065");
        assert_eq!(row_text(&[b], 0, p(16, 16, 1)), "00000000  41");
        // 全 1 的字节在每一种进制下都不能溢出各自位宽。
        for base in HEX_BASES {
            let text = row_text(&[0xff], 0, p(base, 16, 1));
            let body = text.trim_start_matches("00000000").trim();
            assert_eq!(body.chars().count(), p(base, 16, 1).digits());
        }
    }

    /// 分组间隙跟着 `group` 走：它管「几个字节连写」，不碰加宽。
    #[test]
    fn the_group_gap_lands_every_group_bytes() {
        let bytes = [0x41u8, 0x42, 0x43, 0x44];
        assert_eq!(row_text(&bytes, 0, p(16, 16, 1)), "00000000  41 42 43 44");
        assert_eq!(row_text(&bytes, 0, p(16, 16, 2)), "00000000  4142 4344");
        assert_eq!(row_text(&bytes, 0, p(16, 16, 4)), "00000000  41424344");
        assert_eq!(row_text(&bytes, 0, p(16, 16, 8)), "00000000  41424344");
    }

    /// `group` 四档在满行上的完整观感——加宽只在**组边界**上发生。
    ///
    /// 注意最后一条：`group = 8` 时 8 个字节连成一片，第 4 个字节后面**不加宽**
    /// （那还在组内）。加宽位置服从 `group`，不是与它并行的第二条判据。
    #[test]
    fn the_division_widens_only_on_group_boundaries() {
        let bytes: Vec<u8> = (0x41..=0x48u8).collect(); // 8 字节，正好两段
        assert_eq!(
            row_text(&bytes, 0, p(16, 16, 1)),
            "00000000  41 42 43 44  45 46 47 48"
        );
        assert_eq!(
            row_text(&bytes, 0, p(16, 16, 2)),
            "00000000  4142 4344  4546 4748"
        );
        assert_eq!(
            row_text(&bytes, 0, p(16, 16, 4)),
            "00000000  41424344  45464748"
        );
        // 8 字节一组：整行连成一片，双格出现在第 8 个字节之后（组边界）。
        assert_eq!(
            row_text(&bytes, 0, p(16, 16, 8)),
            "00000000  4142434445464748"
        );
        // 15 个字节（尾行不满 16 列）：最后一个字节之后不留空格。
        let short: Vec<u8> = (0x41..=0x4Fu8).collect();
        for group in HEX_GROUP_CHOICES {
            let text = row_text(&short, 0, p(16, 16, group));
            assert!(!text.ends_with(' '), "group={group} 的行尾多了空格：{text:?}");
        }
    }

    /// 列数 / 分组 / 进制一起决定一行多宽，`full_row_chars` 与 `row_text` 必须同源。
    #[test]
    fn the_row_width_matches_the_rendered_row() {
        for base in HEX_BASES {
            for columns in HEX_COLUMNS_CHOICES {
                for group in HEX_GROUP_CHOICES {
                    let params = p(base, columns, group);
                    let full: Vec<u8> = (0..columns).map(|i| i as u8).collect();
                    let text = row_text(&full, 0, params);
                    assert_eq!(
                        text.chars().count(),
                        full_row_chars(params),
                        "{params:?} 的排版宽度与 full_row_chars 不一致"
                    );
                    // 像素宽必须是字符宽的整数倍：字号变了只缩放，不改排版。
                    assert!((needed_width(8.0, params) - 8.0 * full_row_chars(params) as f32).abs() < 0.01);
                }
            }
        }
    }

    /// 分隔符的两条判据（`group` 组边界 + `DIVISION` 加宽）在 `row_text` 里也一致：
    /// **把渲染出来的那一行解析回来**，逐个 `k` 数它后面实际有几个空格。
    ///
    /// 为什么不用 `row.find(&byte)` 定位：偏移列本身就是八个 hex 字符，`00` / `01`
    /// 这些短字节会在前缀里先命中，于是「找到的位置」根本不是那个字节。改成从
    /// 字节区的第一个字符起按 `digits` 走格子，定位就不再依赖字符串内容。
    ///
    /// 四档 `group` 全跑：只测 `group = 2` 的话，`DIVISION` 的加宽位置恰好都落在
    /// 组边界上，那条「或」的实现漏掉加宽也照样绿——`group = 8` 才分得开。
    #[test]
    fn the_separator_rule_is_the_documented_one() {
        for group in HEX_GROUP_CHOICES {
            let params = p(16, 16, group);
            let row = row_text(&(0..16u8).collect::<Vec<_>>(), 0, params);
            // 跳过「偏移 8 位 + 两个空格」，只解析字节区。
            let row = row.strip_prefix("00000000  ").expect("满行以偏移开头");
            let chars: Vec<char> = row.chars().collect();
            let mut at = 0usize;
            for k in 1..=16usize {
                at += params.digits();
                let mut spaces = 0usize;
                while at + spaces < chars.len() && chars[at + spaces] == ' ' {
                    spaces += 1;
                }
                // 两种情形都是零格，但理由不同：行尾不留空格；组内连写时
                // 还没到组边界，加宽也排不上。
                let want = if k == 16 || k % group != 0 {
                    0
                } else if k % DIVISION == 0 {
                    2
                } else {
                    1
                };
                assert_eq!(
                    spaces, want,
                    "group={group} 第 {k} 个字节之后应有 {want} 个空格：{row:?}"
                );
                at += spaces;
            }
            assert_eq!(at, chars.len(), "group={group} 解析消耗的字符数应与行宽相同");
        }
    }

    /// **菜单 `Group Chars` 的每一档都必须看得见**：四档渲染出的行两两不同。
    ///
    /// 这是 I5/S4「假开关」的判据，也是我自己翻过的车：`sep_after` 一度把
    /// 「组内连写」与「组间加宽」拍平成「或」，出厂 `division = 4` 下
    /// `group = 4` 与 `group = 8` 逐字符相同——菜单四档里有两档点了屏幕不变。
    /// 只断言「第 k 个字节后面几格」不够（那只是实现的镜像），所以这里比的是
    /// **整行文本**，并且逐档写出它应该长什么样。
    #[test]
    fn every_group_choice_renders_a_different_row() {
        let bytes: Vec<u8> = (0..16u8).collect();
        let rows: Vec<(usize, String)> = HEX_GROUP_CHOICES
            .iter()
            .map(|&group| {
                let row = row_text(&bytes, 0, p(16, 16, group));
                // 跳过「偏移 8 位 + 两个空格」，只比字节区。
                let body = row
                    .strip_prefix("00000000  ")
                    .expect("满行以偏移开头")
                    .to_string();
                (group, body)
            })
            .collect();

        // 一、两两不同：四档必须画出四种样子。
        for i in 0..rows.len() {
            for j in (i + 1)..rows.len() {
                assert_ne!(
                    rows[i].1, rows[j].1,
                    "group={} 与 group={} 画出了同一行——菜单里有一档是假开关",
                    rows[i].0, rows[j].0
                );
            }
        }

        // 二、逐档钉住形状（比「两两不同」更能说明它到底是什么样）。
        let expect = [
            // group=1：字节各占两位、之间一格；第 4 / 8 / 12 个之后两格。
            (1usize, "00 01 02 03  04 05 06 07  08 09 0A 0B  0C 0D 0E 0F"),
            // group=2：两字节连写，组间一格；第 4 / 8 / 12 个之后两格。
            (2, "0001 0203  0405 0607  0809 0A0B  0C0D 0E0F"),
            // group=4：四字节连写；4 又正好是加宽周期，于是段间两格。
            (4, "00010203  04050607  08090A0B  0C0D0E0F"),
            // group=8：八字节连写，只在第 8 个之后两格——第 4 个之后是 **0 格**。
            (8, "0001020304050607  08090A0B0C0D0E0F"),
        ];
        for ((group, body), (want_group, want)) in rows.iter().zip(expect) {
            assert_eq!(*group, want_group, "档位表的顺序变了");
            assert_eq!(body, want, "group={group} 的字节区形状不对");
        }

        // 三、专门钉住那条最容易写反的：group=8 时第 4 个字节之后没有空格。
        let g8 = &rows[3].1;
        assert!(
            g8.starts_with("0001020304"),
            "group=8 前五个字节必须连写：{g8:?}"
        );
        assert!(
            !g8.contains("00010203  04"),
            "group=8 不许在第 4 个字节后加宽：{g8:?}"
        );
    }

    // ---- 视口 ----

    fn log_with(bytes: &[u8]) -> RawLog {
        let mut log = RawLog::new();
        log.ring.push(bytes);
        log
    }

    /// 贴底：`anchor = None` 时最后一行的结尾必须正好是 `end()`——
    /// 既不能漏掉尾行，也不能多画一行空白。
    #[test]
    fn following_the_tail_ends_exactly_at_the_last_byte() {
        for len in [1usize, 16, 17, 40, 160] {
            let log = log_with(&vec![0x5a; len]);
            let rows = 4;
            let lines = log.visible(rows, HexParams::default());
            let last = lines.last().expect("有字节就至少有一行");
            assert_eq!(
                last.0 + last.1.len() as u64,
                log.ring.end(),
                "len={len} 时最后一行没有收在 end()"
            );
            assert!(lines.len() <= rows, "len={len} 时画出了超过视口的行");
            // 行首偏移必须连续、每行不超过一行的字节数。
            for (i, (off, bytes)) in lines.iter().enumerate() {
                assert_eq!(*off, lines[0].0 + i as u64 * 16, "行偏移不连续");
                assert!(bytes.len() <= 16);
            }
        }
    }

    /// 回看：往上滚一行就是往上挪一整行字节，滚回底部自动回到跟随。
    #[test]
    fn scrolling_moves_by_whole_rows_and_returns_to_following() {
        let mut log = log_with(&(0..200u32).map(|i| i as u8).collect::<Vec<_>>());
        let params = HexParams::default();
        log.scroll_rows(2, 4, params);
        let top = log.anchor.expect("滚上去之后必须记一个锚点");
        assert_eq!(top.get(), log.tail_anchor(4, params).get() - 2 * 16);

        log.scroll_rows(100, 4, params); // 一路滚到顶
        assert_eq!(log.anchor, Some(ByteOffset::new(0)), "滚到环首就停住");

        log.scroll_rows(-100, 4, params); // 一路滚回底
        assert_eq!(log.anchor, None, "回到底部要变回跟随，而不是停在固定偏移");
    }

    /// 洪水之后贴底：视口落在**尾巴**上，不是环首。
    ///
    /// 环首是 `first()`（丢弃点），若视口错落在那里，用户看到的会是**最旧**
    /// 的那一段——而洪水场景下最要紧的恰恰是最新那几个字节。
    #[test]
    fn a_flooded_ring_follows_the_tail_not_the_head() {
        let mut log = RawLog::new();
        log.ring.push(&vec![0u8; crate::demo::BIN_OVERFLOW_BYTES]);
        let params = HexParams::default();
        let rows = 21;
        let lines = log.visible(rows, params);
        assert_eq!(lines.len(), rows);
        assert_eq!(log.ring.first(), 4096, "丢弃的正好是夹具多给的那 4096");
        assert_eq!(
            lines[0].0,
            log.ring.end() - rows as u64 * params.columns as u64
        );
        assert!(lines[0].0 > log.ring.first(), "视口不该停在环首");
        let last = lines.last().expect("有字节就有行");
        assert_eq!(last.0 + last.1.len() as u64, log.ring.end(), "尾行收在 end()");
    }

    /// 环比视口还小时的退化：`tail_anchor` 落在环首，滚动不越界。
    #[test]
    fn a_short_ring_scrolls_within_itself() {
        let mut log = log_with(b"abc");
        let params = HexParams::default();
        assert_eq!(log.tail_anchor(4, params).get(), 0);
        log.scroll_rows(3, 4, params);
        assert_eq!(log.anchor, None, "装不满一屏时无处可滚");
    }

    /// 换列数之后同一个字节仍落在同一个偏移上：锚点是**字节**偏移，不是行号。
    #[test]
    fn the_anchor_survives_a_column_change() {
        let mut log = log_with(&(0..200u8).collect::<Vec<_>>());
        log.scroll_rows(3, 4, HexParams::default());
        let anchor = log.anchor.expect("滚上去之后必须记一个锚点");
        let wide = log.visible(4, p(16, 32, 1));
        assert_eq!(wide[0].0, anchor.get(), "换列数不该让视口跳到别处");
    }

    // ---- 文本出口 ----

    /// 复制走的是同一份 `visible()`：行数一致、每行以 `\n` 结尾。
    #[test]
    fn the_copied_text_matches_the_visible_rows() {
        let log = log_with(&(0..50u8).collect::<Vec<_>>());
        let params = HexParams::default();
        let rows = log.visible(4, params);
        let text = visible_hex_text(&log, 4, params);
        assert_eq!(text.lines().count(), rows.len());
        assert!(text.ends_with('\n'), "每行都要以换行结尾");
        for line in text.lines() {
            assert!(line.starts_with(|c: char| c.is_ascii_hexdigit()));
        }
    }

    // ---- tap ----

    /// `drain` 把通道里的字节全搬进环；没挂 tap 时是空操作。
    #[test]
    fn draining_moves_every_chunk_into_the_ring() {
        let mut log = RawLog::new();
        assert_eq!(log.drain(), 0, "没挂 tap 时什么都不该发生");

        let (tx, rx) = channel();
        log.attach(HexTap::new(rx));
        tx.send(b"hello".to_vec()).expect("送第一块");
        tx.send(b" world".to_vec()).expect("送第二块");
        assert_eq!(log.drain(), 11);
        assert_eq!(log.ring.take(0, 11), b"hello world".to_vec());
        assert_eq!(log.drain(), 0, "已经空了");

        // 摘掉 tap 之后环里的内容**不清**：断开之后还该能看断开前那一段。
        log.detach();
        tx.send(b"late".to_vec()).ok();
        assert_eq!(log.drain(), 0);
        assert_eq!(log.ring.end(), 11);
    }
}
