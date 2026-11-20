//! `center.rs` 的单测（由 `#[path]` 挂回 `overlays/center.rs`）。
//!
//! 拆出来的原因见 `verify.sh` 的行数门禁：`center.rs` 的代码行已逼近 500 上限，
//! 而内联 `mod tests` 也算在生产文件里。测试本身不属于「这个文件画什么」，
//! 挪到兄弟文件既保住门禁，也让绘制代码更好读。

use super::*;

#[test]
fn seven_schemes_match_windterm_names() {
    let names: Vec<String> = (0..7).map(|i| scheme(i).unwrap().theme_name).collect();
    assert_eq!(names, BUILTIN_SCHEME_NAMES);
    assert!(scheme(7).is_none(), "超出 7 套应返回 None");
}

#[test]
fn the_seven_names_resolve_to_five_palettes() {
    // 名字 7 个、调色板只有 5 套：`powershell` 复用 `cmd`（Windows 控制台），
    // `routeros` 复用 `linux`（VGA）。这条断言把该事实钉住——原名
    // `schemes_have_distinct_ansi_palettes` 只比了 cmd 与 linux 一对，名字承诺
    // 「互不相同」而实现只验一对，正是「测试名比测试强」的典型。
    // 现在两个方向都咬：把 powershell 改成独立取值 → 套数变 6 红；把任意两套
    // 改成同值 → 套数变 4 红。共享组只允许这两对，新增一对也红。
    let names = BUILTIN_SCHEME_NAMES;
    assert_eq!(names.len(), 7);
    let palettes: Vec<Palette> = (0..names.len()).map(|i| scheme(i).unwrap()).collect();

    for &(a, b) in &[(0usize, 5usize), (2, 6)] {
        assert_eq!(
            palettes[a].ansi, palettes[b].ansi,
            "{} 与 {} 应共用同一套 ANSI 调色板",
            names[a], names[b]
        );
    }

    let distinct = (0..palettes.len())
        .filter(|&a| (0..a).all(|b| palettes[b].ansi != palettes[a].ansi))
        .count();
    assert_eq!(distinct, 5, "7 个名字应只解析出 5 套不同调色板（7 - 2 组复用）");
}
