//! `confirm_paste` 的单测：判据真值表 + 预览截断（`#[path]` 挂在模块下）。

use super::*;

#[test]
fn the_gate_only_looks_at_the_screen_the_tab_is_on() {
    // 主屏：只看 main_flag。备用屏：只看 alt_flag。两条互不牵连——
    // 在主屏打开了确认，不该让 vim 里的粘贴也开始弹窗。
    assert!(paste_needs_confirm(false, true, false));
    assert!(!paste_needs_confirm(false, false, true));
    assert!(paste_needs_confirm(true, false, true));
    assert!(!paste_needs_confirm(true, true, false));
    // 出厂两个都是 false = 现状（粘贴立刻写进 PTY）。
    assert!(!paste_needs_confirm(false, false, false));
    assert!(!paste_needs_confirm(true, false, false));
}

#[test]
fn the_preview_shows_the_first_lines_and_reports_the_rest() {
    let text = (1..=25)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let p = preview(&text);
    assert_eq!(p.total_lines, 25);
    assert_eq!(p.hidden_lines, 5, "没显示的行数要如实报出来");
    assert!(p.shown.starts_with("line 1\n"));
    assert!(p.shown.ends_with("line 20"), "只显示前 20 行：{}", p.shown);
    assert!(!p.shown.contains("line 21"));
    assert!(!p.char_cut);
}

#[test]
fn one_enormous_line_is_cut_by_the_char_cap_not_by_the_line_cap() {
    // base64 那种「一行几 MB」：按行数截不住，必须由字符上限兜底。
    let text = "x".repeat(5000);
    let p = preview(&text);
    assert_eq!(p.total_lines, 1);
    assert_eq!(p.hidden_lines, 0, "还是一行，没有「更多行」可报");
    assert_eq!(p.shown.chars().count(), 800);
    assert!(p.char_cut, "显示层要能说明「这一行还没显示完」");
}

#[test]
fn the_preview_does_not_lie_about_short_text() {
    let p = preview("echo hi\nls -la");
    assert_eq!(p.shown, "echo hi\nls -la");
    assert_eq!(p.hidden_lines, 0);
    assert!(!p.char_cut);
    // 空粘贴也读成一行：否则标题会说「0 行」。
    let empty = preview("");
    assert_eq!(empty.total_lines, 1);
    assert_eq!(empty.hidden_lines, 0);
    assert!(empty.shown.is_empty());
}
