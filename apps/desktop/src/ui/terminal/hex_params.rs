//! hex 渲染器的**参数与排版**：[`HexParams`] 一份、分隔符规则一份、一行多宽。
//!
//! 从 `bin_spec.rs` 拆出来的理由只有一条（R9 的单文件代码行上限 500）：那个
//! 文件同时装着「字节环」与「一行怎么排版」两件事。拆法按职责，不按行数：
//! **本文件回答「一行长什么样」，`bin_spec.rs` 回答「有哪些字节可画」**，
//! `bin.rs` 才是把两者接起来的绘制。
//!
//! # 五个参数，两个来源（这是本轮最容易写反的地方）
//!
//! [`HexParams`] 有两个来源，分界是用户定死的：
//!
//! | 谁 | 存在哪 | 落盘 | 生效范围 |
//! |----|--------|------|----------|
//! | 设置 ▸ Binary 页 | `Prefs.hex_*` + `Prefs.alternate_column_bg` | 是 | **新会话** |
//! | `View ▸ Hex *` | `Tab.hex` | 否 | 只改当前会话 |
//!
//! 所以类型本身**不知道**自己是从盘上来的还是从菜单来的，它只是一份数值；
//! 夹值只在这里发生一次（[`HexParams::from_prefs`]），两侧共用。
//!
//! **斑马线（[`HexParams::zebra`]）也在这份参数里**，与那四个数同一个来源、
//! 同一个作用域。2026-11-11 之前它是窗口上的一个 bool（`TerminalWindow::
//! alternate_column_bg`），于是同一个设置页上五个同类参数有两套语义：四个数
//! 只影响新标签，而勾掉那一格**当场**就改掉了所有已开标签的画法。收进这里之后
//! 五个参数一律「设置页改默认值 → 只影响新标签」，`View ▸ Hex *` 改当前标签。
//!
//! # 四个排版参数各管什么
//!
//! - `base`：一个字节用几进制写（`2..=16`，菜单给 2 / 7 / 8 / 10 / 16 五个快捷档）；
//! - `columns`：一行几个字节（`1..=64`，菜单给 8 / 16 / 24 / 32 四个快捷档）；
//! - `group`：每几个字节**连写**成一组（`1..=columns`，菜单给 1 / 2 / 4 / 8）；
//! - `division`：每逢第几个字节把分隔符**加宽**成两格（`1..=columns`，没有菜单项，
//!   只有设置页那一行 `devision columns`）。
//!
//! 第五个参数 `zebra` 不管排版：它只决定那层「相邻 `division` 段交替的底色」
//! 画不画（绘制在 `bin.rs`）。它跟组一起走的是 `division` 而不是 `group`——
//! 那一格的标签写的就是 `Alternate column background color`，而这套参数里
//! 「column」唯一的指称是 `devision columns`。
//!
//! 两条规则是嵌套的（见 [`sep_after`]）：加宽只发生在**组边界**上，组内连写
//! 一律零格。`group = 8` 时第 4 个字节后面不加宽——那还在组内。
//!
//! 菜单那几张表是**快捷档**，不是合法值全集：设置页的 `DragValue` 可以填任意
//! 整数，五档只是「替你填一个常用数」。所以 `from_prefs` 判的是区间，不是会员资格。

/// 组间加宽的出厂周期：每逢第 `division` 个整倍数，分隔符从一格变两格。
///
/// 它现在**是** [`HexParams`] 的字段（`division`），这个常量只剩「出厂值 /
/// 越界回落值」一个用途——用户可从设置 ▸ Binary 页改成任意 `1..=columns`。
pub(in crate::ui) const DIVISION: usize = 4;

/// `base` 的合法区间（含两端）。上界 16 不是技术限制而是**可读性限制**：
/// 一个字节在 16 进制下两位、在 2 进制下八位，再往上（BASE32/36）一行就排不下。
pub(in crate::ui) const BASE_RANGE: std::ops::RangeInclusive<u32> = 2..=16;

/// `columns` 的合法区间（含两端）。64 列配 2 进制已经是 600 多个字符，
/// 再宽就纯粹是「画到隔壁面板上」了。
pub(in crate::ui) const COLUMNS_RANGE: std::ops::RangeInclusive<usize> = 1..=64;

/// 偏移列宽（8 位大写 hex，见 `bin_spec::row_text`）。
///
/// 偏移是**字节序号**，与 [`HexParams::base`] 无关——用户换显示进制是想换
/// 「一个字节怎么写」，不是想换「第几个字节」。混在一起会让同一个位置
/// 在两种进制下显示成两个偏移，回看时对不上。
pub(in crate::ui) const OFFSET_DIGITS: usize = 8;

/// `View ▸ Hex Base` 的五档（视觉基线 `view/02`：`2 / 7 / 8 / 10 / 16`）。
///
/// `7` 不是笔误：参考截图里就是它（放大 4× 复核过）。它是**快捷档**之一，
/// 不是合法值全集——`from_prefs` 认的是 [`BASE_RANGE`] 区间。
pub(in crate::ui) const HEX_BASES: [u32; 5] = [2, 7, 8, 10, 16];

/// `View ▸ Hex Columns` 的四档（视觉基线 `view/03`：`8 / 16 / 24 / 32 Columns`）。
pub(in crate::ui) const HEX_COLUMNS_CHOICES: [usize; 4] = [8, 16, 24, 32];

/// `View ▸ Hex Group Chars` 的四档（视觉基线 `view/05`：`1 / 2 / 4 / 8 Chars`）。
///
/// 注意它管的不是「加宽」而是「连写」：`group = 1` 时字节之间一格、每逢
/// 第 `division` 个字节加宽成两格（经典 dump）；`group = 2` 时两字节紧贴成 `4142`。
/// 加宽那件事归 `division`，与这个档位正交。
pub(in crate::ui) const HEX_GROUP_CHOICES: [usize; 4] = [1, 2, 4, 8];

/// hex 渲染器的参数：四个排版数 + 一格斑马线。
///
/// 取值**收在本类型里夹一次**：prefs 是手可改的 JSON，设置页的 `DragValue`
/// 也能被键盘输入任何数，`base = 3` 或 `columns = 0` 必须在进入渲染前就落回
/// 合法区间，否则格式化代码会各写一套兜底。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) struct HexParams {
    /// 一个字节用几进制写（2..=16）。
    pub base: u32,
    /// 每行几个字节（1..=64）。
    pub columns: usize,
    /// 每几个字节连写成一组（组内不留空格，1..=columns）。
    pub group: usize,
    /// 每几个字节把分隔符加宽成两格（1..=columns）。
    pub division: usize,
    /// 相邻 `division` 段要不要交替铺一层淡底色（斑马线）。
    ///
    /// 它**不是**排版参数，但必须跟另外四个待在一起：它同样是「每条会话一份」，
    /// 同样只在建标签那一刻从 `Prefs.alternate_column_bg` 取初值。留在窗口上
    /// 就等于给同一个设置页造出两套语义——四个数只影响新标签，它却当场改掉
    /// 所有已开标签（那是 2026-11-11 修掉的行为）。
    ///
    /// 取值没有区间可夹（`bool` 没有非法态），所以 [`HexParams::from_prefs`]
    /// 只把它原样带过来；prefs 那一侧的「老文件没这项」由 serde 的
    /// `#[serde(default = "default_true")]` 兜底，不需要 `0` 哨兵。
    pub zebra: bool,
}

impl Default for HexParams {
    /// 出厂值 = 视觉基线设置页上那四个数：16 进制 / 16 列 / 每字节一组 / 每 4 字节加宽；
    /// 斑马线出厂**勾上**（`Prefs::default().alternate_column_bg == true`，视觉基线
    /// 那一格就是勾着的）。
    ///
    /// 两处必须一致：`Default` 是「没有 prefs 时的兜底」，出厂 prefs 是「有 prefs
    /// 但没这一项时的兜底」，两者给出不同的画法就会让「删掉 prefs.json」看起来
    /// 像一个功能改动。
    fn default() -> Self {
        Self {
            base: 16,
            columns: 16,
            group: 1,
            division: DIVISION,
            zebra: true,
        }
    }
}

impl HexParams {
    /// 由四个数加一格斑马线造一份：**越界回落出厂值**（`0` 哨兵也走这条路）。
    ///
    /// `0` 是「老文件没这一项」的哨兵（与本仓库其余 prefs 字段同一套写法），
    /// 所以这里不能把 0 当成合法值往渲染器里传。
    ///
    /// 判的是**区间**而不是「在不在菜单那几张表里」：设置页的数字框能填 20 列，
    /// 那是个合法值，不该因为菜单里没有这一档就被打回 16。
    ///
    /// `group` / `division` 的上界是 `columns` 而不是 64：一组比一整行还宽，
    /// 那一行就没有任何分隔符，「分组」这个设置等于没生效。
    ///
    /// `zebra` 原样带走——它不是「会被夹坏的数」，而是「用户勾没勾」。
    /// 把五个参数收进同一个构造入口是有意的：建标签那条路上只要漏掉一项，
    /// 新标签就会静默退回出厂值，而那正是「设置页改了没反应」的形状。
    pub(in crate::ui) fn from_prefs(
        base: u32,
        columns: usize,
        group: usize,
        division: usize,
        zebra: bool,
    ) -> Self {
        let d = HexParams::default();
        let columns = within(columns, COLUMNS_RANGE, d.columns);
        Self {
            base: within_u32(base, BASE_RANGE, d.base),
            columns,
            group: within(group, 1..=columns, d.group),
            // 出厂值 4 在 `columns < 4` 的行上比列数还大，夹到列数即可
            // （那种行里根本没有第 4 个字节，加宽本来就画不出来）。
            division: within(division, 1..=columns, d.division).min(columns).max(1),
            zebra,
        }
    }

    /// 一个字节占几个字符位：该进制写下 255 需要几位。
    ///
    /// 定宽而不是「按值取最短」：列对齐是 hex 视图存在的意义，`0xFF` 与 `0x0`
    /// 占不同的宽度会让整张表歪掉。
    ///
    /// 通用算法而不是 `match`：`base` 现在是 2..=16 的自由值，写死五档会在
    /// 用户填 5 进制时给出错误位宽（5³ = 125 ≤ 255，要 4 位）。
    pub(in crate::ui) fn digits(self) -> usize {
        digits_for(self.base)
    }

    /// 按窗宽夹一次列数：取**装得下的最大**列数（不超过参数原本的列数）。
    ///
    /// 为什么必须夹：`32 Columns` 配 `base 2` 时一行是
    /// `10 + 32*8 + 分隔符 ≈ 274` 个字符（≈2000px），而终端窗格通常只有 630px。
    /// 不夹的话 dump 会直接画到右边的 Diff / Session 面板上——那不是「宽」，
    /// 是画到别人的地盘里。
    ///
    /// 只夹 `columns`：其余三个决定的是「一个字节怎么写」「组内连写几个」，
    /// 它们只改变系数；列数是唯一会让整行成倍翻的那一维。另外三个字段原样
    /// 带走，用户点 `Columns ▸ 8` 之后不该偷偷把进制也退回 16。
    ///
    /// 斑马线同理原样带走：夹列数的本质是「这一行排不下」，跟「底色画不画」
    /// 没有任何关系。它跟着走也是 [`HexParams::fitted`] 的返回值能被当作
    /// 「本帧渲染参数」直接交给渲染器的前提——否则 `pane.rs` 还得在夹完之后
    /// 再从别处捞一次开关，那就又造出了第二个读取点。
    ///
    /// 逐列往下试，而不是只试菜单那四档：用户在设置页填了 20 列，窗宽只装得下
    /// 17 时该画 17 列，而不是跳到 16 或 24。
    ///
    /// 一列都装不下时取 1 列：极端窄窗由绘制层截断，但那时至少不是我们自己
    /// 选了「画一半」。
    pub(in crate::ui) fn fitted(self, cell_w: f32, avail_w: f32) -> HexParams {
        let mut columns = self.columns.max(1);
        while columns > 1 && needed_width(cell_w, Self { columns, ..self }) > avail_w {
            columns -= 1;
        }
        Self { columns, ..self }
    }
}

/// 应用一个 `View ▸ Hex *` 旋钮：**只换被点的那一维，其余各维原样带走**。
///
/// 这是菜单路径的全部数学。它的签名就是那条分界线的证据：`cur: HexParams` 进、
/// `HexParams` 出，**手上既没有 `Prefs` 也没有 `Tab`**——所以「菜单改了不落盘」
/// 不是一个约定，而是这个函数在类型层面做不到写盘。调用方
/// （`ui/effects.rs::set_tab_hex`）要把结果写回**活动标签**，那是另一层的事。
///
/// 斑马线跟着原样带走：菜单上没有它的旋钮（它是设置 ▸ Binary 页那一格），
/// 所以这里既不该改它，也不该把它退回出厂——点一次 `Base ▸ 2` 顺手把用户
/// 关掉的斑马线打开，就是「改一个旋钮动了另一个」。
///
/// 为什么要过一遍 [`HexParams::from_prefs`]：`columns` 是 `group` / `division`
/// 的上界。用户把列数从 32 改成 8 之后，`group = 8` 仍然合法但 `division = 4`
/// 也可能越界（列数 2 时）——夹一次就都对齐了，不用在每个旋钮上各写一段。
pub(in crate::ui) fn apply_knob(cur: HexParams, knob: term_app::shell::HexKnob) -> HexParams {
    use term_app::shell::HexKnob;
    let (base, columns, group) = match knob {
        HexKnob::Base(b) => (b, cur.columns, cur.group),
        HexKnob::Columns(c) => (cur.base, c, cur.group),
        HexKnob::Group(g) => (cur.base, cur.columns, g),
    };
    HexParams::from_prefs(base, columns, group, cur.division, cur.zebra)
}

/// 把 `v` 夹进 `range`；越界回落 `fallback`（`0` 哨兵也走这条路）。
fn within(v: usize, range: std::ops::RangeInclusive<usize>, fallback: usize) -> usize {
    if range.contains(&v) {
        v
    } else {
        fallback
    }
}

/// [`within`] 的 `u32` 版本。两个类型各一份而不是泛型：本文件只需要两种，
/// 泛型化要牵进 `PartialOrd` / `Copy` 两个约束，读起来比两行多得多。
fn within_u32(v: u32, range: std::ops::RangeInclusive<u32>, fallback: u32) -> u32 {
    if range.contains(&v) {
        v
    } else {
        fallback
    }
}

/// 某进制写下 255 需要几位。
///
/// [`HexParams::digits`] 与 `byte_text` 共用它：两处各写一遍的话，非常规进制
/// （5 / 7 / 12 …）迟早有一处漏掉，表现是补零补错、整张表歪掉。
pub(super) fn digits_for(base: u32) -> usize {
    let base = base.clamp(2, 36);
    let mut n = 1usize;
    let mut cap = base;
    while cap <= u8::MAX as u32 {
        n += 1;
        cap = cap.saturating_mul(base);
    }
    n
}

/// 一个字节按 `base` 写成定宽大写文本。
///
/// 只有一条通用路径：`base` 是自由值（2..=16），用 `format!` 特判 2/8/10/16
/// 会让 5 / 7 / 12 这些值落到最后那个 `_` 分支，而那条分支就是下面这句——
/// 于是「特判」只是多了一份要同步维护的位宽表。7 进制仍是 `122`、16 进制仍是
/// `41`、2 进制仍是 `01000001`，单测逐个钉住。
pub(super) fn byte_text(b: u8, base: u32) -> String {
    let mut out = digits_in_base(b as u32, base);
    let width = digits_for(base);
    while out.len() < width {
        out.insert(0, '0');
    }
    out
}

/// 任意进制的定宽无关写法（大写 0-9A-Z，最少一位）。
fn digits_in_base(mut value: u32, base: u32) -> String {
    const ALPHABET: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let base = base.clamp(2, 36) as usize;
    let mut out = String::new();
    loop {
        out.push(ALPHABET[(value as usize) % base] as char);
        value /= base as u32;
        if value == 0 {
            break;
        }
    }
    out.chars().rev().collect()
}

/// 写完第 `k` 个字节（`k` 从 1 数）之后要几个空格。
///
/// **分隔符规则只有这一处**：`row_text`（画）、`full_row_chars`（算宽）、
/// `byte_char_at`（斑马线定位）三处都调它。各写一遍的话，加宽位置迟早有一处
/// 走偏，表现是「底色块和文字错开一格」这种最难查的对齐 bug。
///
/// 两条判据是**嵌套**的，不是「或」：
///
/// ```text
/// k % group != 0        → 0 格   组内连写，加宽也不例外
/// 否则 k % division == 0 → 2 格   段与段之间加宽
/// 否则                   → 1 格   普通段边界
/// ```
///
/// 为什么必须是嵌套：`group` 管「几个字节连成一段」，`division` 管「段与段之间
/// 多宽」。拍平成「或」之后，出厂值 `division = 4` 下 `group = 4` 与 `group = 8`
/// 会**逐字符相同**（第 4 个字节后面永远被加宽成两格，8 从来没有机会连写满），
/// 于是菜单 `Group Chars` 四档里有两档点了屏幕不变——那正是 I5/S4 要抓的假开关。
/// 组还没结束时谈不上「段与段之间」，所以加宽只能在组边界上发生。
///
/// 行尾不留空格由调用方保证（`k == columns` 时不要调它）。
pub(in crate::ui) fn sep_after(k: usize, params: HexParams) -> usize {
    if !k.is_multiple_of(params.group.max(1)) {
        0
    } else if k.is_multiple_of(params.division.max(1)) {
        2
    } else {
        1
    }
}

/// 一整行（满 `params.columns` 字节）的**字符数**。
///
/// 排版只有这一份算法：`bin_spec::row_text` 用它预分配、[`needed_width`] 用它
/// 算像素宽、[`HexParams::fitted`] 用它判「这一档装不装得下」。
pub(in crate::ui) fn full_row_chars(params: HexParams) -> usize {
    let mut n = OFFSET_DIGITS + 2 + params.columns * params.digits();
    // 分隔符挂在「写完第几个字节」上（k 从 1 数），最后一个字节后面永远没有。
    for k in 1..params.columns {
        n += sep_after(k, params);
    }
    n
}

/// 第 `i` 个字节（0 起）在本行里的**字符**起点。
///
/// 斑马线要把底色画在「文字下面第几个字符」上，而字节是整行一次 `text()`
/// 画出来的，没有逐字节的 rect 可拿。所以这里给出字符偏移，绘制层乘 cell 宽
/// 就是像素位置。它和 `row_text` 共用 [`sep_after`]——两者一旦走偏，底色块
/// 就会和字节错开。
pub(in crate::ui) fn byte_char_at(i: usize, params: HexParams) -> usize {
    let mut at = OFFSET_DIGITS + 2;
    for k in 1..=i {
        at += params.digits();
        // 注意 k 是「写完第 k 个字节」的下标：求第 i 个字节的起点时，
        // 要算完前 i 个字节各自的位宽与它们后面的分隔符。
        at += sep_after(k, params);
    }
    at
}

/// 一整行 hex 需要多少像素宽。窗口窄于它时由 [`HexParams::fitted`] 先夹列数。
pub(in crate::ui) fn needed_width(cell_w: f32, params: HexParams) -> f32 {
    full_row_chars(params) as f32 * cell_w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(base: u32, columns: usize, group: usize) -> HexParams {
        HexParams {
            base,
            columns,
            group,
            division: DIVISION,
            zebra: true,
        }
    }

    /// 越界 / 0 哨兵回落出厂值——不许把非法值带进渲染器。
    #[test]
    fn out_of_range_params_fall_back_to_defaults() {
        assert_eq!(
            HexParams::from_prefs(0, 0, 0, 0, true),
            HexParams::default()
        );
        // base 17 超出 2..=16，columns 0 / 65 超出 1..=64。
        assert_eq!(
            HexParams::from_prefs(17, 16, 1, 4, true),
            HexParams::default()
        );
        assert_eq!(
            HexParams::from_prefs(16, 65, 1, 4, true),
            HexParams::default()
        );
        // group / division 的上界是**列数**：8 列里不可能 9 个字节一组。
        assert_eq!(HexParams::from_prefs(16, 8, 9, 4, true).group, 1);
        assert_eq!(HexParams::from_prefs(16, 8, 4, 9, true).division, 4);
    }

    /// 区间内的任意值都合法，不要求「在菜单那几张表里」。
    ///
    /// 这是「base 不再是五档会员制」的判据：设置页的 `DragValue` 能填 5 / 12，
    /// 它们必须原样活下来，否则用户填完又被静默改回 16。
    #[test]
    fn any_value_inside_the_range_survives_instead_of_snapping_to_a_preset() {
        assert_eq!(HexParams::from_prefs(5, 16, 1, 4, true).base, 5);
        assert_eq!(HexParams::from_prefs(12, 16, 1, 4, true).base, 12);
        assert_eq!(HexParams::from_prefs(16, 20, 1, 4, true).columns, 20);
        assert_eq!(HexParams::from_prefs(16, 1, 1, 1, true).columns, 1);
        assert_eq!(HexParams::from_prefs(16, 64, 64, 64, true).columns, 64);
        // 菜单那几张表仍是合法值（它们是快捷档，不是白名单）。
        for base in HEX_BASES {
            assert_eq!(HexParams::from_prefs(base, 16, 1, 4, true).base, base);
        }
        for columns in HEX_COLUMNS_CHOICES {
            assert_eq!(
                HexParams::from_prefs(16, columns, 1, 4, true).columns,
                columns
            );
        }
        for group in HEX_GROUP_CHOICES {
            assert_eq!(HexParams::from_prefs(16, 16, group, 4, true).group, group);
        }
        // 档位表必须升序（菜单按它排、`fitted` 的旧实现也吃过这个假设）。
        assert!(HEX_COLUMNS_CHOICES.windows(2).all(|w| w[0] < w[1]));
        assert!(HEX_GROUP_CHOICES.windows(2).all(|w| w[0] < w[1]));
        assert!(HEX_BASES.windows(2).all(|w| w[0] < w[1]));
    }

    /// 斑马线**没有**可夹的区间：勾没勾原样穿过 `from_prefs`。
    ///
    /// 这条挡的是「顺手把 bool 也做成区间判断」这类改动：`false` 不是非法值，
    /// 把它当 0 哨兵翻成 `true`（出厂值）就等于用户关不掉那层底色，而设置页
    /// 那一格会表现为「勾掉、重开设置页又勾上了」。
    #[test]
    fn the_zebra_flag_passes_through_untouched() {
        // 勾上就得是勾上，关掉就得是关掉。
        assert!(HexParams::from_prefs(16, 16, 1, 4, true).zebra);
        assert!(!HexParams::from_prefs(16, 16, 1, 4, false).zebra);
        // 越界的排版数**不**连累斑马线：夹的是数，不是开关。
        assert!(!HexParams::from_prefs(0, 0, 0, 0, false).zebra);
        // 出厂值是勾上的（与 `Prefs::default().alternate_column_bg` 一致）。
        assert!(HexParams::default().zebra);
        assert_eq!(
            HexParams::default().zebra,
            term_runtime::prefs::Prefs::default().alternate_column_bg,
            "`Default` 与出厂 prefs 必须给出同一种画法，否则删掉 prefs.json 会像一个功能改动"
        );
    }

    /// 位宽是**通用算法**：255 在该进制下需要几位就算几位。
    #[test]
    fn the_digit_width_is_computed_not_tabulated() {
        assert_eq!(digits_for(2), 8, "255 = 11111111₂");
        assert_eq!(digits_for(7), 3, "7³ = 343 > 255");
        assert_eq!(digits_for(8), 3, "8³ = 512 > 255");
        assert_eq!(digits_for(10), 3);
        assert_eq!(digits_for(16), 2);
        // 5 进制：5³ = 125 ≤ 255 < 625，所以 4 位。写死五档的话这里会给出 2。
        assert_eq!(digits_for(5), 4);
        assert_eq!(digits_for(12), 3, "12² = 144 ≤ 255 < 12³，所以 3 位");
        // 每个合法进制下 255 都不溢出位宽，0 也不短一位。
        for base in BASE_RANGE {
            let width = digits_for(base);
            assert_eq!(byte_text(0xff, base).chars().count(), width, "{base} 进制");
            assert_eq!(byte_text(0, base).chars().count(), width, "{base} 进制补零");
        }
    }

    /// 7 进制是真要画的：`255 = 513₇` 三位；`65 = 122₇`。
    ///
    /// 视觉基线 `view/02` 里那一档就是 `7`，不是笔误，所以它必须有单测——
    /// 否则哪天有人「顺手把 7 改成 8」不会有任何东西拦他。
    #[test]
    fn the_seven_base_writes_three_digits() {
        assert_eq!(byte_text(0, 7), "000");
        assert_eq!(byte_text(7, 7), "010");
        assert_eq!(byte_text(65, 7), "122"); // 65 = 1*49 + 2*7 + 2
        assert_eq!(byte_text(255, 7), "513", "7 进制下一个字节最多 3 位");
        assert_eq!(p(7, 16, 1).digits(), 3, "位宽与 8 / 10 进制同一档");
    }

    /// 窗宽夹取：取装得下的**最大**列数；且不小于 1。
    ///
    /// 数字是算出来的（base 16 / group 1 / division 4 的满行字符数）：8 列 34、
    /// 16 列 60、24 列 86、32 列 112 个字符；乘 8px 得 272 / 480 / 688 / 896。
    #[test]
    fn fitted_picks_the_widest_columns_that_fit() {
        let params = HexParams::default();
        for columns in HEX_COLUMNS_CHOICES {
            let one = HexParams { columns, ..params };
            assert_eq!(
                full_row_chars(one),
                match columns {
                    8 => 34,
                    16 => 60,
                    24 => 86,
                    _ => 112,
                },
                "{columns} 列的满行字符数变了——先确认是哪一处排版改动，再改这里"
            );
        }
        assert_eq!(params.fitted(8.0, 2000.0).columns, 16, "不放大，只往下夹");
        assert_eq!(params.fitted(8.0, 480.0).columns, 16, "正好装得下要算装得下");
        assert_eq!(params.fitted(8.0, 479.0).columns, 15, "逐列往下试，不跳档");
        // 用户把列数调大之后，窗宽仍然是唯一的夹取依据。
        let wide = HexParams { columns: 32, ..params };
        assert_eq!(wide.fitted(8.0, 896.0).columns, 32);
        assert_eq!(wide.fitted(8.0, 895.0).columns, 31);
        // 600px 装得下 20 列（73 字符 × 8 = 584），21 列是 616 —— 逐列试才对得上。
        assert_eq!(wide.fitted(8.0, 600.0).columns, 20);
        // 极端窄窗：至少留 1 列，不画「半行」也不画 0 列。
        assert_eq!(params.fitted(8.0, 10.0).columns, 1);
        // 另外三个字段原样带走：夹列数不许顺手把进制 / 分组 / 加宽退回出厂。
        // base 2 时一列 8 个字符，300px 只够 3 列（34 字符 × 8 = 272）。
        let odd = p(2, 32, 4);
        let fitted = odd.fitted(8.0, 300.0);
        assert_eq!((fitted.base, fitted.group, fitted.division), (2, 4, DIVISION));
        assert!(fitted.zebra, "夹列数不碰斑马线：排不下跟画不画底色是两件事");
        assert_eq!(fitted.columns, 3);
    }

    /// 加宽只发生在**组边界**上：组内的 `division` 整倍数不加宽。
    ///
    /// 这是「反假开关」的机制那一半（看得见的那一半在
    /// `bin_spec::every_group_choice_renders_a_different_row`）。我曾经把这两条
    /// 判据拍平成「或」，于是出厂 `division = 4` 下 `group = 4` 与 `group = 8`
    /// 渲染逐字符相同，菜单四档有两档点了没反应。谁再拍平，这里先红。
    #[test]
    fn a_group_that_has_not_ended_swallows_the_division_gap() {
        let params = |group| HexParams {
            group,
            ..HexParams::default()
        };
        // group = 1：每个字节都是组边界，于是 `division` 的整倍数照样加宽。
        assert_eq!(sep_after(1, params(1)), 1);
        assert_eq!(sep_after(4, params(1)), 2);
        assert_eq!(sep_after(8, params(1)), 2);
        // group = 8：第 4 个字节还在组内 → **0 格**，不是 2 格。
        assert_eq!(sep_after(4, params(8)), 0, "组内连写，加宽也不例外");
        assert_eq!(sep_after(7, params(8)), 0);
        // 第 8 个字节既是组边界又是 4 的整倍数 → 两格。
        assert_eq!(sep_after(8, params(8)), 2);
        // group = 4：第 4 个字节既结束一组又撞上加宽周期 → 两格。
        assert_eq!(sep_after(4, params(4)), 2);
        assert_eq!(sep_after(8, params(4)), 2);
        assert_eq!(sep_after(2, params(4)), 0);
        // group = 2：组边界一格，撞上加宽周期两格。
        assert_eq!(sep_after(2, params(2)), 1);
        assert_eq!(sep_after(4, params(2)), 2);
        assert_eq!(sep_after(1, params(2)), 0);
        // 行尾由调用方跳过，函数本身照算（`k == columns` 时不该看它的返回值）。
        assert_eq!(sep_after(16, params(1)), 2);

        // 字符起点与分隔符必须互相咬合（斑马线靠它定位）。
        let d = HexParams::default();
        assert_eq!(byte_char_at(0, d), OFFSET_DIGITS + 2);
        // 1 个字节（2 位）之后是组边界（group=1）→ 1 格。
        assert_eq!(byte_char_at(1, d), OFFSET_DIGITS + 2 + 2 + 1);
        // 4 个字节之后是加宽位置 → 2 格。
        assert_eq!(byte_char_at(4, d), OFFSET_DIGITS + 2 + 4 * 2 + 1 + 1 + 1 + 2);
    }

    /// 菜单旋钮只动被点的那一维：**其余各维必须原样带走**。
    ///
    /// 这条是两层模型里「菜单改的是当前会话」的最小可测部分：`apply_knob` 的
    /// 签名里没有 `Prefs`、没有盘，所以「改一个旋钮顺手把别的旋钮也写回盘上」
    /// 这类事在类型层面就做不到；这里再把「别的维不会跟着变」钉死。
    ///
    /// 「别的维」包括斑马线：用户手动关掉底色之后点一下 `Group ▸ 4`，底色
    /// 不该自己回来。`zebra: false` 是这条的判据——用出厂值 `true` 做夹具的话，
    /// 「旋钮把它重置成出厂值」这个 bug 会正好被掩掉。
    #[test]
    fn a_knob_moves_only_its_own_dimension() {
        use term_app::shell::HexKnob;
        let cur = HexParams {
            base: 16,
            columns: 24,
            group: 2,
            division: 8,
            zebra: false,
        };

        let base = apply_knob(cur, HexKnob::Base(2));
        assert_eq!((base.base, base.columns, base.group, base.division), (2, 24, 2, 8));
        assert!(!base.zebra, "点 Base 不许顺手把用户关掉的底色打开");

        let columns = apply_knob(cur, HexKnob::Columns(8));
        assert_eq!(
            (columns.base, columns.columns, columns.group, columns.division),
            (16, 8, 2, 8)
        );
        assert!(!columns.zebra);

        let group = apply_knob(cur, HexKnob::Group(4));
        assert_eq!(
            (group.base, group.columns, group.group, group.division),
            (16, 24, 4, 8)
        );
        assert!(!group.zebra);

        // 越界由 `from_prefs` 一处夹掉：列数缩到 2 时 8 字节一组越界了。
        let shrunk = apply_knob(cur, HexKnob::Columns(2));
        assert_eq!(shrunk.columns, 2);
        assert!(shrunk.group <= shrunk.columns, "组宽不能比一整行还宽");
        assert!(shrunk.division <= shrunk.columns, "加宽周期同理");
        assert!(!shrunk.zebra, "夹列数不许顺手把用户关掉的底色打开");
    }
}
