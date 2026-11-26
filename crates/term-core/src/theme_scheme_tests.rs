//! 七套内置终端配色方案的逐条判据（`View ▸ Syntax Schemes` 的菜单项）。
//! 名字 → 调色板的唯一真相是 `theme::builtin_scheme`，菜单 ID 的映射在
//! `term-app::parse`，这里只钉住「七个名字各有一套真色、未知名字没有」。

use crate::theme::{builtin_scheme, BUILTIN_SCHEME_NAMES};

#[test]
fn the_seven_menu_names_match_the_builtin_table() {
    assert_eq!(
        BUILTIN_SCHEME_NAMES,
        &["cmd", "dumb", "linux", "macos", "none", "powershell", "routeros"]
    );
}

#[test]
fn every_scheme_has_its_own_sixteen_colours_and_name() {
    let mut sources = Vec::new();
    for name in BUILTIN_SCHEME_NAMES {
        let palette = builtin_scheme(name).unwrap_or_else(|| panic!("{name} 缺内置调色板"));
        assert_eq!(palette.theme_name, *name);
        assert_eq!(palette.ansi_source, format!("built-in scheme '{name}'"));
        assert_eq!(palette.ansi.len(), 16);
        // 每套都定义前景色；背景恒为终端底色（黑），与 WindTerm 行为一致。
        assert_ne!(palette.default_fg, palette.default_bg, "{name} 前景不能等于背景");
        sources.push(palette.ansi);
    }
    // cmd 与 powershell、linux 与 routeros 共用色表是有意的（参考别名），
    // 其余几套必须给出不同的 16 色，否则菜单切了等于没切。
    assert_ne!(sources[0], sources[1]);
    assert_ne!(sources[0], sources[3]);
    assert_ne!(sources[1], sources[4]);
}

#[test]
fn an_unknown_scheme_is_not_silently_replaced_by_a_known_one() {
    assert!(builtin_scheme("").is_none());
    assert!(builtin_scheme("bogus").is_none());
    assert!(builtin_scheme("CMD").is_none(), "方案名大小写敏感");
}
