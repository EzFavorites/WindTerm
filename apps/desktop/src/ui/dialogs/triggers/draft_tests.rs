//! Triggers 对话框的草稿往返判据（由 `#[path]` 挂回 `triggers/mod.rs`）。
//!
//! 审查发现的 bug 是「Cancel 之后运行期规则已经改了」：编辑直接落在
//! `win.triggers` 上，Cancel 只关窗。这里把「打开 → 编辑 → Cancel / Ok」
//! 的整条往返用生产同一对函数（begin_draft / finish_draft）钉死。
//!
//! 注意帧首 `swap` 的方向：`draft` 局部变量拿到的是**已提交**版本，
//! `live`（= `win.triggers`）才是这一帧被编辑的草稿。

use super::*;

fn rule(name: &str) -> Rule {
    Rule {
        id: name.to_owned(),
        name: name.to_owned(),
        description: String::new(),
        kind: TriggerKind::SendText,
        config: RuleConfig::default(),
        on: term_core::trigger::TriggerOn::Output,
        pattern: String::new(),
        literal: true,
        case_sensitive: true,
        cooldown_secs: 0,
        idle_secs: 0,
        actions: Vec::new(),
        enabled: true,
    }
}

#[test]
fn cancel_restores_the_committed_rules_and_discards_the_draft() {
    let committed = vec![rule("a"), rule("b")];
    let mut view = View::default();
    let mut live = committed.clone();

    // 打开：草稿是已提交规则的克隆；swap 之后 live 是正在编辑的草稿。
    let mut committed_saved = begin_draft(&mut view, &live);
    std::mem::swap(&mut live, &mut committed_saved);
    assert_eq!(committed_saved, committed, "换出来的是已提交版本");

    // 用户在草稿里删一条、加一条。
    live.remove(0);
    live.push(rule("c"));
    assert_ne!(live, committed, "编辑只发生在草稿上");

    // Cancel：把已提交版本换回去，草稿作废。
    finish_draft(&mut view, committed_saved, &mut live, false, true);
    assert_eq!(live, committed, "Cancel 之后运行期规则必须原样");
    assert!(view.draft.is_none(), "关窗后不该留着草稿");

    // 下次打开重新从已提交版本克隆，上次取消的编辑不会回来。
    let next = begin_draft(&mut view, &live);
    assert_eq!(next, committed);
}

#[test]
fn ok_keeps_the_edited_rules_as_the_new_committed_snapshot() {
    let committed = vec![rule("a")];
    let mut view = View::default();
    let mut live = committed.clone();
    let mut committed_saved = begin_draft(&mut view, &live);
    std::mem::swap(&mut live, &mut committed_saved);
    live.push(rule("b"));

    // Ok：`apply` 已经把草稿写回 prefs 并 reload，live 就是新的已提交版本。
    finish_draft(&mut view, committed_saved, &mut live, true, true);
    assert_eq!(live.len(), 2, "Ok 之后新规则留在运行期");
    assert_eq!(view.committed.as_ref(), Some(&live));
    assert!(view.draft.is_none());
}

#[test]
fn an_unchanged_frame_keeps_editing_the_same_draft() {
    let committed = vec![rule("a")];
    let mut view = View::default();
    let mut live = committed.clone();
    let mut committed_saved = begin_draft(&mut view, &live);
    std::mem::swap(&mut live, &mut committed_saved);

    // 这一帧没有点 Ok / Cancel：换回已提交版本，但草稿留在 View 里。
    finish_draft(&mut view, committed_saved, &mut live, false, false);
    assert_eq!(live, committed);
    assert_eq!(view.draft.as_ref(), Some(&vec![rule("a")]));

    // 下一帧接着编辑，不会丢字。
    let mut committed_saved = begin_draft(&mut view, &live);
    std::mem::swap(&mut live, &mut committed_saved);
    live.push(rule("b"));
    finish_draft(&mut view, committed_saved, &mut live, false, false);
    let draft = begin_draft(&mut view, &live);
    assert_eq!(draft.len(), 2, "跨帧的草稿内容必须还在");
}

#[test]
fn a_committed_change_while_closed_invalidates_the_stale_draft() {
    let mut view = View::default();
    let live = vec![rule("a")];
    let mut draft = begin_draft(&mut view, &live);
    draft.push(rule("stale"));
    // 没点 Cancel / Ok 就关窗（脚本关窗 / Esc）：把草稿留在 View 里。
    view.draft = Some(draft);

    // 关闭期间规则在别处变了；下次打开必须重新克隆，不能带回 stale。
    let changed = vec![rule("z")];
    let next = begin_draft(&mut view, &changed);
    assert_eq!(next, changed);
}
