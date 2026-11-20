//! `SessionDialogKind::Properties`（属性面板）的分派语义。
//!
//! 归属 `dialogs/new_session.rs` 的 `mod tests`（用 `#[cfg(test)] #[path]` 挂回去，
//! 与 `session_form.rs` 的 `session_form_*_tests.rs` 同一惯例）。
//!
//! 为什么这些断言值得单独存在：属性面板与 `Edit` 的字段、预填、写回完全一样，
//! 唯一差别是**保存后不连接**。那一步没有任何画面证据（截图只拍到「按了之后
//! 长什么样」，拍不到「按了之后连没连」），所以只能由单测守。四个变体逐个断言
//! 而不是只测 `Properties`，是因为最容易犯的错不是「属性面板连了」，而是
//! 「为了让属性面板别连，顺手把 `Edit` 也改成了不连」——老行为逐字节不变。

use super::*;

/// `Properties` 是「编辑已存在会话」的一种：写回 `sessions[index]`，不是新增。
#[test]
fn properties_writes_back_the_same_record() {
    // 保存路径靠 `edit_index()` 决定「覆盖还是新增」：返回 `None` 会被当成
    // 新建、在列表末尾 push 一条重名记录。属性面板必须是 `Some(3)`。
    assert_eq!(
        SessionDialogKind::Properties { index: 3 }.edit_index(),
        Some(3),
        "属性面板保存时覆盖 sessions[3]，不新增记录"
    );
    // 与 `Edit` 取同一条；`New` / `Default` 保持 `None`（老行为）。
    assert_eq!(SessionDialogKind::Edit { index: 3 }.edit_index(), Some(3));
    assert_eq!(SessionDialogKind::New { group: None }.edit_index(), None);
    assert_eq!(SessionDialogKind::Default.edit_index(), None);
}

/// 只有 `New` / `Edit` 会在保存后发起连接；`Properties` / `Default` 不会。
#[test]
fn only_new_and_edit_reconnect_after_save() {
    // 老行为：新建即连、改完重连。这两条是回归红线，改动前后必须一致。
    assert!(SessionDialogKind::New { group: None }.should_connect());
    assert!(SessionDialogKind::Edit { index: 0 }.should_connect());
    // 死代码转正：`should_connect()` 在 D3 之前无人调用，`save_session_dialog`
    // 尾部无条件连接。现在它是那一段唯一的开关。
    assert!(
        !SessionDialogKind::Properties { index: 0 }.should_connect(),
        "属性面板只改参数，不该把正在跑的会话踢掉重连"
    );
    assert!(!SessionDialogKind::Default.should_connect());
}

/// 标题沿用 `Edit`、主按钮说「只保存」。断言具体字符串，防日后静默漂移。
#[test]
fn properties_looks_like_edit_but_promises_no_connect() {
    let props = SessionDialogKind::Properties { index: 2 };
    let edit = SessionDialogKind::Edit { index: 2 };
    let new = SessionDialogKind::New { group: None };
    let default = SessionDialogKind::Default;

    // 标题：用户看到的都是「这条会话的属性」，差别只在收尾连不连——那不该
    // 体现在窗口标题上，所以与 `Edit` 同字。
    assert_eq!(props.title(), "Session Properties");
    assert_eq!(props.title(), edit.title(), "属性面板与 Edit 同标题");
    assert_eq!(new.title(), "New Session");
    assert_eq!(default.title(), "Edit Default Session");

    // 按钮：沿用文件里既有的 `Save`（`Default` 也是它），不另造花哨词。
    // 这里的按钮按下去就关窗落盘，所以不用 `Apply`。
    assert_eq!(props.primary_button(), "Save");
    assert_eq!(props.primary_button(), default.primary_button());
    assert_eq!(edit.primary_button(), "Save & Connect");
    assert_eq!(new.primary_button(), "Connect");

    // 文案不得与行为相反：不连接的模式，按钮上不许出现 `Connect`。
    assert!(
        !props.primary_button().contains("Connect"),
        "属性面板不连接，按钮文案不能承诺连接：{}",
        props.primary_button()
    );
    assert!(!default.primary_button().contains("Connect"));
}
