//! tmux 布局串（`%layout-change` 的第二个字段）的解析与几何投影。
//!
//! 布局串是**外部输入**：它由远端 tmux 的 `layout.c` 生成，只要本地解析器
//! 有一个 `unwrap` / 越界索引，恶意或损坏的会话就能把客户端打挂。所以这里是
//! 全 `Option` 的递归下降，深度有上限、数字有上限、坐标必须落在父块内。
//!
//! 语法（`man tmux` 的 FORMATS 段 + 可观测的 `%layout-change` 输出）：
//!
//! ```text
//! layout   := checksum ',' node
//! checksum := 4 位十六进制
//! node     := W 'x' H ',' X ',' Y tail
//! tail     := '{' node (',' node)* '}'   // 水平切分（子块从左到右）
//!           | '[' node (',' node)* ']'   // 垂直切分（子块从上到下）
//!           | ',' pane-id                // 叶子
//! ```
//!
//! 例：`b25d,80x24,0,0{40x24,0,0,1,39x24,41,0,2}` 是两个并排窗格，
//! 左边 40 列、右边 39 列，中间那 **1 列**是窗格边框。
//!
//! 因此「面积守恒」不能写成 `叶子面积之和 == 根面积`——真实布局里每次切分都要
//! 让出 1 格给边框。这里采用的判据是 tmux 自己的不变量：
//!
//! - 切分轴：`Σ 子块长度 + (n - 1) == 父块长度`，且首个子块贴着父块原点、
//!   后续子块紧接前一个的末尾 + 1；
//! - 交叉轴：每个子块的长度与原点必须与父块完全一致。
//!
//! 这条判据同时覆盖了「子块越界」与「面积之和不守恒」两类畸形输入。

use crate::tmux::Command;

/// 布局树的最大嵌套深度。真实 tmux 的窗格树受窗口尺寸限制，远达不到这个量级；
/// 设上限是为了让 `{{{{…` 这种输入在有限栈内被拒绝，而不是栈溢出。
const MAX_DEPTH: usize = 32;

/// 单边字符格的理智上限。tmux 报的是**字符格**，与 PTY 的 cols/rows 同单位，
/// 不需要换算；超过 1000 列的终端在工程上不存在，钳掉避免下游把脏值当真的窗口尺寸。
pub const MAX_DIM: u16 = 1000;

/// 布局树的一个节点。中间层（切分）也可能带坐标，但**没有** pane id。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Leaf { id: u32, w: u16, h: u16, x: u16, y: u16 },
    /// `{ … }`：水平切分，子块从左到右。
    HSplit { w: u16, h: u16, x: u16, y: u16, children: Vec<Node> },
    /// `[ … ]`：垂直切分，子块从上到下。
    VSplit { w: u16, h: u16, x: u16, y: u16, children: Vec<Node> },
}

impl Node {
    fn rect(&self) -> Rect {
        match self {
            Node::Leaf { w, h, x, y, .. }
            | Node::HSplit { w, h, x, y, .. }
            | Node::VSplit { w, h, x, y, .. } => Rect { x: *x, y: *y, w: *w, h: *h },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x: u16,
    y: u16,
    w: u16,
    h: u16,
}

impl Rect {
    fn contains(&self, child: Rect) -> bool {
        // 先升到 u32：`x + w` 在 u16 上会溢出，溢出后的比较会放过越界子块。
        let (px, py, pw, ph) = (u32::from(self.x), u32::from(self.y), u32::from(self.w), u32::from(self.h));
        let (cx, cy, cw, ch) = (u32::from(child.x), u32::from(child.y), u32::from(child.w), u32::from(child.h));
        cx >= px && cy >= py && cx + cw <= px + pw && cy + ch <= py + ph
    }
}

/// 一份**已通过结构校验**的布局。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub checksum: u32,
    pub width: u16,
    pub height: u16,
    pub root: Node,
}

impl Layout {
    /// 按 pane id 升序返回 `(id, x, y, w, h)`。
    ///
    /// 升序是刻意的：树序会随切分顺序变化，而上基线需要稳定顺序，否则同一份
    /// 语义在两次「先切左还是先切右」之后会画出不同顺序的窗格。
    pub fn panes(&self) -> Vec<(u32, u16, u16, u16, u16)> {
        let mut out = Vec::new();
        collect_leaves(&self.root, &mut out);
        out.sort_by_key(|p| p.0);
        out
    }

    /// 结构不变量是否成立：每层切分按 1 格边框守恒、叶子非空、根尺寸与字段一致。
    ///
    /// 手工构造的 `Layout`（投影层可能这么干）不会被 `parse_layout` 校验，
    /// 所以这条检查不是同义反复：它能在使用前把「宽高写错」「子块重叠」查出来。
    pub fn is_consistent(&self) -> bool {
        self.width == self.root.rect().w && self.height == self.root.rect().h && consistent(&self.root)
    }
}

fn collect_leaves(node: &Node, out: &mut Vec<(u32, u16, u16, u16, u16)>) {
    match node {
        Node::Leaf { id, w, h, x, y } => out.push((*id, *x, *y, *w, *h)),
        Node::HSplit { children, .. } | Node::VSplit { children, .. } => {
            for child in children {
                collect_leaves(child, out);
            }
        }
    }
}

fn consistent(node: &Node) -> bool {
    match node {
        Node::Leaf { w, h, .. } => *w > 0 && *h > 0,
        Node::HSplit { children, .. } => {
            children.len() >= 2
                && fills(node.rect(), children, true)
                && children.iter().all(consistent)
        }
        Node::VSplit { children, .. } => {
            children.len() >= 2
                && fills(node.rect(), children, false)
                && children.iter().all(consistent)
        }
    }
}

/// 子块是否恰好铺满父块（切分轴留 `n - 1` 格边框，交叉轴零让位）。
fn fills(parent: Rect, children: &[Node], horizontal: bool) -> bool {
    let mut cursor = if horizontal { u32::from(parent.x) } else { u32::from(parent.y) };
    for child in children {
        let r = child.rect();
        if horizontal {
            if r.x as u32 != cursor || r.y != parent.y || r.h != parent.h {
                return false;
            }
            cursor += u32::from(r.w);
        } else {
            if r.y as u32 != cursor || r.x != parent.x || r.w != parent.w {
                return false;
            }
            cursor += u32::from(r.h);
        }
        cursor += 1; // 与下一个子块之间的边框
    }
    // 循环里给最后一个子块也多加了 1 格边框，减回去就是父块的末端。
    cursor - 1
        == if horizontal {
            u32::from(parent.x) + u32::from(parent.w)
        } else {
            u32::from(parent.y) + u32::from(parent.h)
        }
}

/// 解析一段布局串。任何畸形输入都返回 `None`，不 panic。
pub fn parse_layout(text: &str) -> Option<Layout> {
    let text = text.trim();
    if text.is_empty() || !text.is_ascii() {
        return None;
    }
    let mut p = Parser { buf: text.as_bytes(), pos: 0 };
    let checksum = p.checksum()?;
    let root = p.node(None, 0)?;
    if p.pos != p.buf.len() {
        return None;
    }
    Some(Layout { checksum, width: root.rect().w, height: root.rect().h, root })
}

/// 把布局声明的总尺寸换算成我方 PTY 的 `(cols, rows)`。
///
/// tmux 布局串里的尺寸本来就是字符格，与 cols/rows 同单位，**不做**单位换算；
/// 只钳到 `1..=MAX_DIM`——0 尺寸的 PTY 没有意义，超宽窗口则视为脏数据。
pub fn layout_to_size(layout: &Layout) -> (u16, u16) {
    (clamp_dim(layout.width), clamp_dim(layout.height))
}

/// 我方窗口变成 `(cols, rows)` 之后要发给 tmux 的 resize 命令。
pub fn resize_command(cols: u16, rows: u16) -> Command {
    Command::Resize { cols: clamp_dim(cols), rows: clamp_dim(rows) }
}

/// 真正改某个窗格尺寸的入口（区别于上面的整窗 `refresh-client`）。
/// 窗格号由调用方从控制流解析出的集合里选，这里只负责钳制尺寸。
pub fn resize_pane_command(pane: u32, cols: u16, rows: u16) -> Command {
    Command::ResizePane { pane, cols: clamp_dim(cols), rows: clamp_dim(rows) }
}

fn clamp_dim(v: u16) -> u16 {
    v.clamp(1, MAX_DIM)
}

struct Parser<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.buf.get(self.pos).copied()
    }

    fn eat(&mut self, want: u8) -> Option<()> {
        if self.peek()? == want {
            self.pos += 1;
            Some(())
        } else {
            None
        }
    }

    /// 十进制无符号数，必须至少一位，且装得进 u16。
    fn number(&mut self) -> Option<u16> {
        let start = self.pos;
        let mut value: u32 = 0;
        while let Some(b @ b'0'..=b'9') = self.peek() {
            value = value.checked_mul(10)?.checked_add(u32::from(b - b'0'))?;
            if value > u32::from(u16::MAX) {
                return None;
            }
            self.pos += 1;
        }
        if self.pos == start {
            return None;
        }
        Some(value as u16)
    }

    /// 恰好 4 位十六进制 + 逗号。
    fn checksum(&mut self) -> Option<u32> {
        let digits = self.buf.get(self.pos..self.pos + 4)?;
        let mut value = 0u32;
        for &b in digits {
            value = value * 16 + u32::from(hex_value(b)?);
        }
        self.pos += 4;
        self.eat(b',')?;
        Some(value)
    }

    fn node(&mut self, parent: Option<Rect>, depth: usize) -> Option<Node> {
        if depth > MAX_DEPTH {
            return None;
        }
        let w = self.number()?;
        self.eat(b'x')?;
        let h = self.number()?;
        self.eat(b',')?;
        let x = self.number()?;
        self.eat(b',')?;
        let y = self.number()?;
        let rect = Rect { x, y, w, h };
        if parent.is_some_and(|p| !p.contains(rect)) {
            return None;
        }

        match self.peek()? {
            b',' => {
                self.pos += 1;
                let id = u32::from(self.number()?);
                Some(Node::Leaf { id, w, h, x, y })
            }
            open @ (b'{' | b'[') => {
                self.pos += 1;
                let close = if open == b'{' { b'}' } else { b']' };
                let mut children = Vec::new();
                loop {
                    children.push(self.node(Some(rect), depth + 1)?);
                    match self.peek()? {
                        b',' => self.pos += 1,
                        c => {
                            self.pos += 1;
                            if c != close {
                                return None;
                            }
                            break;
                        }
                    }
                }
                if children.len() < 2 || !fills(rect, &children, open == b'{') {
                    return None;
                }
                Some(if open == b'{' {
                    Node::HSplit { w, h, x, y, children }
                } else {
                    Node::VSplit { w, h, x, y, children }
                })
            }
            _ => None,
        }
    }
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_two_pane_horizontal_layout_becomes_two_rects() {
        let layout = parse_layout("b25d,80x24,0,0{40x24,0,0,1,39x24,41,0,2}").expect("layout");
        assert_eq!(layout.checksum, 0xb25d);
        assert_eq!((layout.width, layout.height), (80, 24));
        assert!(matches!(layout.root, Node::HSplit { .. }));
        assert_eq!(layout.panes(), vec![(1, 0, 0, 40, 24), (2, 41, 0, 39, 24)]);
        assert!(layout.is_consistent());
    }

    #[test]
    fn a_nested_vertical_split_keeps_absolute_coordinates() {
        let text = "9a3d,120x40,0,0[120x20,0,0{59x20,0,0,1,60x20,60,0,2},120x19,0,21,3]";
        let layout = parse_layout(text).expect("layout");
        assert!(matches!(layout.root, Node::VSplit { .. }));
        assert_eq!(
            layout.panes(),
            vec![(1, 0, 0, 59, 20), (2, 60, 0, 60, 20), (3, 0, 21, 120, 19)]
        );
        assert!(layout.is_consistent());
    }

    #[test]
    fn panes_are_sorted_by_id_not_by_tree_order() {
        // 树序是 7 在左、3 在右；接口承诺按 id 升序，否则上基线会随切分顺序抖动。
        let layout = parse_layout("aaaa,80x24,0,0{40x24,0,0,7,39x24,41,0,3}").expect("layout");
        assert_eq!(layout.panes(), vec![(3, 41, 0, 39, 24), (7, 0, 0, 40, 24)]);
    }

    #[test]
    fn a_single_leaf_layout_is_valid() {
        let layout = parse_layout("0f0f,80x24,0,0,1").expect("layout");
        assert_eq!(layout.panes(), vec![(1, 0, 0, 80, 24)]);
        assert!(layout.is_consistent());
    }

    #[test]
    fn surrounding_whitespace_is_tolerated() {
        assert!(parse_layout("  b25d,80x24,0,0,1\n").is_some());
    }

    #[test]
    fn malformed_layouts_are_rejected_without_panicking() {
        let cases = [
            "",
            "   ",
            "80x24,0,0,1",                                // 缺 checksum
            "b25,80x24,0,0,1",                            // checksum 只有 3 位
            "b25dd,80x24,0,0,1",                          // checksum 5 位（第 5 位不是逗号）
            "zzzz,80x24,0,0,1",                           // checksum 非十六进制
            "b25d80x24,0,0,1",                            // checksum 后缺逗号
            "b25d,80x24,0,0",                             // 叶子缺 pane id
            "b25d,80x24,0,0,",                            // pane id 为空
            "b25d,80x24,0,0,x",                           // pane id 非数字
            "b25d,65536x24,0,0,1",                        // 尺寸溢出 u16
            "b25d,80x24,0,0{40x24,0,0,1",                 // 括号不闭合
            "b25d,80x24,0,0[40x24,0,0,1,39x24,41,0,2}",   // 括号类型不配对
            "b25d,80x24,0,0{40x24,0,0,1,39x24,41,0,2}]",  // 多余收尾
            "b25d,80x24,0,0{}",                           // children 为空
            "b25d,80x24,0,0{40x24,0,0,1}",                // 只有一个 child
            "b25d,80x24,0,0{50x24,0,0,1,40x24,51,0,2}",   // 越界 + 面积超父块
            "b25d,80x24,0,0{40x24,0,0,1,39x24,42,0,2}",   // 中间留 2 格缝隙
            "b25d,80x24,0,0{40x23,0,0,1,39x24,41,0,2}",   // 交叉轴高度不一致
            "b25d,80x24,0,0{40x24,0,0,1,38x24,41,0,2}",   // 面积少 1 格（不是边框）
            "b25d,80x24,5,5{40x24,0,0,1,39x24,41,0,2}",   // 子块起点在父块外
            "b25d,80x24,0,0{40x24,0,0,1,39x24,41,0,2}x",  // 尾部多余字节
            "b25d,80x24,0,0,1 extra",                     // 叶子后还有内容
            "b25d,80x24,0,0{40x24,0,0,1,,39x24,41,0,2}",  // 空元素
        ];
        for case in cases {
            assert_eq!(parse_layout(case), None, "应当拒绝: {case:?}");
        }
    }

    #[test]
    fn a_pathologically_nested_layout_is_refused_instead_of_overflowing_the_stack() {
        let mut text = String::from("aaaa,");
        text.push_str(&"1x1,0,0{".repeat(400));
        text.push_str("1x1,0,0,1");
        assert_eq!(parse_layout(&text), None);
    }

    #[test]
    fn a_hand_built_layout_with_overlapping_children_is_inconsistent() {
        let layout = Layout {
            checksum: 0,
            width: 80,
            height: 24,
            root: Node::HSplit {
                w: 80,
                h: 24,
                x: 0,
                y: 0,
                children: vec![
                    Node::Leaf { id: 1, w: 40, h: 24, x: 0, y: 0 },
                    Node::Leaf { id: 2, w: 39, h: 24, x: 42, y: 0 },
                ],
            },
        };
        assert!(!layout.is_consistent());
        // panes() 不校验，只投影——校验失败的责任在上层（调用 is_consistent）。
        assert_eq!(layout.panes().len(), 2);
    }

    #[test]
    fn a_zero_sized_pane_is_inconsistent_but_still_parseable() {
        let layout = parse_layout("aaaa,80x24,0,0{0x24,0,0,1,79x24,1,0,2}").expect("layout");
        assert!(!layout.is_consistent());
    }

    #[test]
    fn layout_size_maps_to_pty_size_with_clamping() {
        let layout = parse_layout("b25d,80x24,0,0,1").expect("layout");
        assert_eq!(layout_to_size(&layout), (80, 24));

        let empty = Layout { checksum: 0, width: 0, height: 0, root: Node::Leaf { id: 1, w: 0, h: 0, x: 0, y: 0 } };
        assert_eq!(layout_to_size(&empty), (1, 1));

        let huge = Layout { checksum: 0, width: 5000, height: 700, root: Node::Leaf { id: 1, w: 5000, h: 700, x: 0, y: 0 } };
        assert_eq!(layout_to_size(&huge), (MAX_DIM, 700));
    }

    #[test]
    fn resize_command_uses_refresh_client_c_and_never_sends_a_zero_size() {
        assert_eq!(resize_command(120, 40).line(), "refresh-client -C 120x40");
        assert_eq!(resize_command(0, 0).line(), "refresh-client -C 1x1");
        assert_eq!(resize_command(50_000, 3).line(), "refresh-client -C 1000x3");
        // 窗格 resize 走 -t %N，尺寸同样钳制。
        assert_eq!(
            resize_pane_command(3, 50_000, 0).line(),
            "resize-pane -t %3 -x 1000 -y 1"
        );
    }

    #[test]
    fn a_layout_change_event_feeds_the_parser() {
        let line = "%layout-change @1 b25d,80x24,0,0{40x24,0,0,1,39x24,41,0,2}";
        let layout = match crate::tmux::parse_line(line) {
            crate::tmux::Event::LayoutChange { window, layout } => {
                assert_eq!(window, 1);
                layout
            }
            other => panic!("期望 LayoutChange，得到 {other:?}"),
        };
        let parsed = parse_layout(&layout).expect("layout");
        assert_eq!(parsed.panes().len(), 2);
        assert_eq!(layout_to_size(&parsed), (80, 24));
    }
}
