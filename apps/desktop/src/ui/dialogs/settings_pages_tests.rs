//! `settings_pages` 的单测：规格表本身的一致性。
//!
//! 与数据分文件是为了单文件行数；`#[path]` 挂回 `settings_pages` 的私有作用域。

use super::*;


    #[test]
    fn all_pages_round_trip_through_index() {
        // `ALL` 是组件基线的唯一页序（左栏另见 `NAV_TOP`）：`SettingsPage(i)`
        // 的下标就是它的下标，所以 index / from_index 必须是一对反函数。
        assert_eq!(SettingsPage::ALL.len(), 21);
        for (i, page) in SettingsPage::ALL.iter().enumerate() {
            assert_eq!(page.index(), i, "{page:?} 的 index 与它在 ALL 里的位置不一致");
            assert_eq!(SettingsPage::from_index(i), Some(*page));
        }
        assert_eq!(SettingsPage::from_index(SettingsPage::ALL.len()), None);
    }

    #[test]
    fn all_and_all_pages_are_the_same_set() {
        // 两张表只差顺序（`ALL_PAGES` 是老的「按枚举声明」表）。加页只更新一张
        // 的话这条会红——它们是同一批页，不许各说各话。
        assert_eq!(SettingsPage::ALL_PAGES.len(), SettingsPage::ALL.len());
        for page in SettingsPage::ALL_PAGES {
            assert!(SettingsPage::ALL.contains(&page), "{page:?} 不在 ALL 里");
        }
    }

    #[test]
    fn the_wired_pages_are_drawn_by_hand_not_by_the_table() {
        // 这些页里有真读数 / 真控件，一旦被搬回表里就会变成灰的——那是功能
        // 回退，不是排版。整页都是真的：Application / Appearance / Caret /
        // CommandHistory / Search / Filer / Binary（Binary 的四个数字框 + 一格
        // 斑马线直接写 `Prefs` 的默认参数，读数取自窗口上的 `hex_defaults` 副本
        // ——五格是同一份 `HexParams`，斑马线也在里面）。
        // Font / Tabs / Terminal 与 `settings_live_text` 的 Text 页是「部分真控件」：
        // 每页只有一两行接了字段，其余行由 `settings_live_*` 借数据表的 `draw_row`
        // 画成灰的，所以它们同样**不能**回到 `page_spec`（回去就整页一起灰）。
        for page in [
            SettingsPage::Application,
            SettingsPage::Appearance,
            SettingsPage::Font,
            SettingsPage::Caret,
            SettingsPage::CommandHistory,
            SettingsPage::Search,
            SettingsPage::Binary,
            SettingsPage::Filer,
            SettingsPage::Text,
            SettingsPage::Tabs,
            SettingsPage::Terminal,
            SettingsPage::QuickBar,
            SettingsPage::Wrap,
            SettingsPage::Highlight,
            SettingsPage::AutoCompletion,
            SettingsPage::Triggers,
            SettingsPage::OneKeys,
        ] {
            assert!(page_spec(page).is_none(), "{page:?} 不该走数据表");
        }
    }

    #[test]
    fn every_other_page_has_a_spec_with_a_title() {
        for page in SettingsPage::ALL_PAGES {
            if page_spec(page).is_none() {
                continue;
            }
            let spec = page_spec(page).unwrap();
            assert!(!spec.title.is_empty(), "{page:?} 的页标题不能为空");
        }
        // 三个组头也是页（视觉基线里点 Terminal / Text 有内容，Widgets 是空页）。
        // `Text` 组头页自 2026-11-10 起是手绘的（部分真控件），`Terminal`
        // 同日也搬去手绘（两个粘贴确认开关接了字段），所以三个组头都不在表里；
        // `Widgets` 仍是空页，仍由数据表兜底。
        assert!(page_spec(SettingsPage::Terminal).is_none());
        assert!(page_spec(SettingsPage::Text).is_none());
        assert!(page_spec(SettingsPage::Widgets).unwrap().blocks.is_empty());
    }

    #[test]
    fn group_titles_are_not_repeated_within_one_page() {
        for page in SettingsPage::ALL_PAGES {
            let Some(spec) = page_spec(page) else { continue };
            let mut titles: Vec<&str> = spec
                .blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Group { title, .. } => Some(*title),
                    _ => None,
                })
                .collect();
            let n = titles.len();
            titles.sort_unstable();
            titles.dedup();
            assert_eq!(titles.len(), n, "{page:?} 里有两个同名分组框");
        }
    }

    #[test]
    fn the_pages_that_cannot_be_wired_say_what_is_missing() {
        // Wrap 仍走数据表、说明要写清「缺的是软换行引擎」；Highlight
        // 2026-11-19 已搬去 live 页（caret line 接真），它的说明在
        // `settings_live_highlight::HIGHLIGHT_NOTE`，不在这里。
        assert!(
            page_note(SettingsPage::Wrap).contains("hard-wraps"),
            "Wrap 页要写清缺的是「内核已按列宽硬折行」这件事"
        );
        // 2026-11-14：这句文案原先谎报「三项都没有渲染器」。`Wrap symbols` 与
        // `Column markers` 2026-11-13 就接线了（`View.ToggleWrapSymbols` /
        // `View.ToggleColumnMarker` → `MarksKnob::WrapSymbols` / `ColumnMarker`），
        // 真正没有画法的只有 `Wrap column`。两条一起钉住，免得下次有人把承诺
        // 往回改宽或改窄。
        assert!(
            page_note(SettingsPage::Wrap).contains("Only Wrap column has no renderer"),
            "Wrap 页只能承认 `Wrap column` 没有渲染器，另两项已经由 View 菜单提供"
        );
        assert!(
            !page_note(SettingsPage::Wrap).contains("no renderer at all"),
            "别再说「三项都没有渲染器」：wrap symbols / column markers 早就接线了"
        );
        // Mouse Tracking 那句旧说明（「没有编码通道」）2026-11-19 已更正：
        // 通道在，缺的是五个事件偏好字段与 DECSET 控制权规则。钉住新说法。
        // Mouse Tracking 2026-11-21 起是 live 页（四类事件掩码 + 两行灰显），
        // 数据表说明与 spec 都已移走；留在这里的旧断言会钉住一个不存在的页面。
        assert!(
            page_spec(SettingsPage::MouseTracking).is_none(),
            "Mouse Tracking 已迁到 settings_live_mouse"
        );
        assert_eq!(page_note(SettingsPage::MouseTracking), "");
        // 2026-11-10：`Proxy` 等八页也换了专句，通用那句不再有任何有内容的页在用。
        // 这里改成**反向**判据：凡是有内容可画的灰页，都不许退回套话——
        // 「没有配置字段」只说了一半（缺哪一层？），要求点名缺口。
        // 还剩 `Widgets` 走 `DISABLED_NOTE`，但它 `blocks` 为空、`draw` 提前返回，
        // 那句话一个像素都不会上屏，所以不在这里的判据范围内。
        for page in SettingsPage::ALL_PAGES {
            let Some(spec) = page_spec(page) else { continue };
            if spec.blocks.is_empty() {
                continue;
            }
            assert_ne!(
                page_note(page),
                DISABLED_NOTE,
                "{page:?} 仍在用「没有配置字段」那句套话，得点名缺的是哪一层"
            );
        }
    }

    /// 2026-11-15 按兼容终端截图清理：Preferences 的页脚只剩 `Close`。那句橙色
    /// 「什么时候生效」连同 `footer_note` 数据表一起删掉了——各页的说明仍由
    /// `page_note` / `settings_live_text::*_NOTE` 在页内给出。这条守的是
    /// 「不许把长提示又挂回页脚」，它是「被 Close 压掉半截」的直接原因。
    #[test]
    fn the_settings_footer_has_no_page_note() {
        let src = include_str!("settings.rs");
        assert!(!src.contains("NOTE_ORANGE"), "橙色页脚提示不许回来");
        assert!(!src.contains("footer_note"), "按页的页脚提示不许回来");
        assert!(!src.contains("REFERENCE_FOOTER"), "参考原文不许挂回页脚");
    }

    /// 视觉基线里这些取值是**照抄**的，不是我们编的默认值；抄错一个字就该红。
    #[test]
    fn the_copied_values_still_match_the_reference() {
        // Proxy 页 2026-11-21 起是 live 页：默认值与可选项在 settings_live_proxy。
        assert!(page_spec(SettingsPage::Proxy).is_none());
        assert_eq!(page_note(SettingsPage::Proxy), "");
        // Binary 页已改成 live 页（读数必须每帧从 `win.hex` 读），不再是数据表里
        // 的常量行。那两处拼写照抄的证据现在在 `settings_live_binary`：`devision`
        // 是视觉基线自己的拼写，改成 `division` 那边就红。
        assert_eq!(
            super::super::settings_live_binary::DEVISION_LABEL,
            "Binary devision columns:"
        );
        assert!(
            page_spec(SettingsPage::Binary).is_none(),
            "Binary 页是手绘的 live 页，不在数据表里"
        );
        let search = page_spec(SettingsPage::Search);
        assert!(search.is_none(), "Search 页是手绘的 live 页，不在数据表里");
        // 六个引擎现在是 `term-runtime` 的一份数据（设置页与网搜共读）。
        // 原先这里断言的是数据表里那张两列表有 6 行——表本身已随 Search 一起退场。
        assert_eq!(term_runtime::search::SEARCH_ENGINES.len(), 6, "六个引擎");
    }

    #[test]
    fn no_disabled_page_claims_a_setting_is_on() {
        // 勾 = 宣称「这项已生效」。灰显页里的控件零读者，所以表里只许出现
        // `DisabledCheck`（空框）或 `Check(_, false)`——一条 `Check(_, true)`
        // 都不许有。有读者的开关走 `settings_live` 手绘页，根本不在这张表里。
        for page in SettingsPage::ALL_PAGES {
            let Some(spec) = page_spec(page) else { continue };
            for block in spec.blocks {
                let rows: &[Row] = match block {
                    Block::Group { rows, .. } => rows,
                    Block::Rows(rows) => rows,
                };
                for row in rows {
                    if let Row::Check(label, true) = row {
                        panic!("{page:?} 的「{label}」灰显却打勾：勾＝宣称已生效，而它零读者");
                    }
                }
            }
        }
    }
