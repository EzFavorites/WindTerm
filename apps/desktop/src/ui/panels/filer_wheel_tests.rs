//! 文件面板滚轮的准入判据（2026-11-14 修「滚轮永远落在文件浏览器」）。
//!
//! 那个 bug 不是「焦点错了」，是这一列**绕过了指针判定**：`InputState` 是
//! 整帧全局的，无条件读 `raw_scroll_delta` 等于把窗口里每一次滚轮都记到本面板。
//! 修复落成 `wheel_rows` 的第二个参数，所以判据钉在这里——它挡的是
//! 「第二个参数又被写成 `true`」这一种回退。
//!
//! 为什么不写 headless 绘制测试：`Filer` 的字段全是私有的，`draw_filer` 一帧
//! 要走 `ensure_filer_session` + `tr()` + 右键菜单 Area，构造成本远高于判据的
//! 覆盖面；而坐标（`list_rect`）是布局算出来的，钉死在测试里比 bug 本身更脆。
//! 面板层面的观感由用户手工验收。

use super::wheel_rows;

/// 指针不在列表区：滚轮一律不算数——哪怕量很大、方向任意。
#[test]
fn a_wheel_outside_the_list_moves_nothing() {
    assert_eq!(wheel_rows(14.0, false), 0);
    assert_eq!(wheel_rows(-140.0, false), 0);
}

/// 指针在列表区：按 `ROW_H`（19）换算成整行。egui 一格滚轮 ≈ 14 点。
#[test]
fn a_wheel_over_the_list_moves_whole_rows() {
    // 14 / 19 = 0.74，四舍五入 1 行：一格滚轮不该被舍成 0，否则滚了没反应。
    assert_eq!(wheel_rows(14.0, true), 1);
    assert_eq!(wheel_rows(-14.0, true), -1);
    // 正好半行向上取整（f32::round 半值远离零），避免「慢慢滚半天不动」。
    assert_eq!(wheel_rows(9.5, true), 1);
    // 没有滚轮时不许凭空产生位移。
    assert_eq!(wheel_rows(0.0, true), 0);
    // 多格按比例放大：38 / 19 = 2。
    assert_eq!(wheel_rows(-38.0, true), -2);
}
