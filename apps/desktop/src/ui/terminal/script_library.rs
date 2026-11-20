//! 「运行脚本」窗口里的两份清单：**脚本库**（读盘）与**最近用过**（读 prefs）。
//!
//! 为什么需要它：D8-4 定了脚本库目录（`profile_dir()/scripts`）之后，窗口里仍然
//! 只有一个路径框——等于要求用户先记住那个目录在哪、里面有哪些文件。绝大多数人
//! 不知道 `~/Library/Application Support/WindTerm/scripts` 在哪。于是左列列出库里
//! 现有的 `*.rhai`，右列列出最近跑过的，点一下就填进路径框。
//!
//! 为什么清单**在打开窗口时取一次**而不是每帧读盘：目录内容在「窗口开着的这段
//! 时间」里不会变（存脚本要先关掉这个窗口再进编辑器），而每帧一次 `read_dir`
//! 会让这一帧的耗时任凭磁盘决定。要刷新就重开窗口——这句话是真的，不是借口。
//!
//! 为什么单开一个文件：`ui/terminal/script.rs` 已经 628 代码行（4j 上限 500，
//! 登记在 `scripts/verify.sh` 的欠账表里）。清单是**独立的判据**（过滤、排序、
//! 空态、点选），与「运行 / 停止」那套状态机没有交集，拆开才能各自被量、各自
//! 被单测。

use crate::ui::*;
use term_core::theme::CHROME_SCALE;
use term_runtime::paths::scripts_dir;

/// 一行的高度。与 `dialogs/quick_commands.rs` 的行同值——两处都是「一列可点的
/// 名字」，行高不同会让人以为是两种控件。
const ROW_H: f32 = 18.0;
/// 列头（`Library` / `Recent`）占的高度。
const HEAD_H: f32 = 16.0;

/// 脚本库里所有 `*.rhai`：`(完整路径, 展示名)`，按展示名排序。
///
/// 库目录不存在（第一次运行、还没存过任何脚本）时给空表——空表在界面上是
/// 「no scripts in library」那一句，不是崩溃。
pub(in crate::ui) fn library() -> Vec<(String, String)> {
    let Some(dir) = scripts_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let paths = entries.filter_map(|e| e.ok()).map(|e| e.path());
    collect(paths)
}

/// `library()` 的**判据部分**（纯函数：只认路径，不碰磁盘）。
///
/// 拆出来的理由与 `push_recent` 同一条：过滤规则（只认 `.rhai`）与排序可以被
/// 没有 profile 目录的单测钉死，而"怎么拿到这些路径"不需要被测。
fn collect<I: IntoIterator<Item = std::path::PathBuf>>(paths: I) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = paths
        .into_iter()
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("rhai"))
        .map(|p| {
            let display = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            (p.to_string_lossy().into_owned(), display)
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// 一行的展示名：文件名（路径放在悬停提示里）。
///
/// 清单只有 220 点宽，完整路径一定被裁掉，而**被裁掉的正好是文件名**——那才是
/// 用户认得出的部分。路径本身仍然有用（同名的 `deploy.rhai` 可能有两个），
/// 所以放进 `on_hover_text`，需要时停在那一行上就能看见。
fn display_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_string())
        // 没有文件名（`/`、`..` 这类）就照原样显示：宁可长，不可空。
        .unwrap_or_else(|| path.to_string())
}

/// 画两份清单，返回本帧被点中的那个路径（`None` = 没点）。
///
/// `current` 是路径框里现在那一串：与之相同的行画成选中底色。**点击只填路径、
/// 不直接开跑**——「点一下就跑」在列表里是危险的（列表就在路径框下面，误点代价
/// 是一次不可撤销的远端执行）。
pub(in crate::ui) fn draw_lists(
    ui: &mut egui::Ui,
    lang: term_app::i18n::Lang,
    library: &[(String, String)],
    recent: &[String],
    current: &str,
    height: f32,
) -> Option<String> {
    let recent_rows: Vec<(String, String)> = recent
        .iter()
        .map(|path| (path.clone(), display_name(path)))
        .collect();
    let mut picked: Option<String> = None;
    ui.columns(2, |cols| {
        if let Some(path) = column(
            &mut cols[0],
            "script_library_list",
            t(lang, "Library"),
            t(lang, "no scripts in library"),
            library,
            current,
            height,
        ) {
            picked = Some(path);
        }
        if let Some(path) = column(
            &mut cols[1],
            "script_recent_list",
            t(lang, "Recent"),
            t(lang, "nothing run yet"),
            &recent_rows,
            current,
            height,
        ) {
            picked = Some(path);
        }
    });
    if picked.is_some() {
        // 路径框已经用旧文本画过这一帧了；不主动要下一帧，新填的路径要等到
        // 用户下一次动鼠标才出现。
        ui.ctx().request_repaint();
    }
    picked
}

/// 一列：列头 + 定高滚动区里的行。
///
/// 高度写死（`height`）而不是「内容多高就多高」：空列与满列必须占同样一块地方，
/// 否则窗口会在「跑过一次脚本」前后跳高一次——底下的按钮跟着上下移动，正是
/// 最容易点错的那种布局。
fn column(
    ui: &mut egui::Ui,
    id: &str,
    title: &str,
    empty: &str,
    rows: &[(String, String)],
    current: &str,
    height: f32,
) -> Option<String> {
    let mut picked = None;
    ui.set_min_height(height);
    // 列头用 `icon` 灰而不是 `hint` 灰（0xA1 vs 0x44）：`hint` 是**占位文字**的色，
    // 0x44 在深底上本来就看不清——英文还勉强，中文小字直接糊成一团。分组标签
    // 要「暗但不隐形」，比正文（0xBB）低一档正好。
    //
    // 字号与行同是 12 而不是更小的 11：中文在 11 点下会被压成一条 8 像素高的
    // 糊字（实测「脚本库」三字认不出笔画），层级只靠颜色分就够了。
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(title)
            .color(crate::theme::chrome(CHROME_SCALE.icon))
            .size(crate::ui::type_scale::SIZE_BODY),
    );
    let list_h = (height - HEAD_H).max(ROW_H);
    // 两份清单同宽同高、行也一样高，只能用**不同的 id** 分开滚动位置；不给 id
    // 的话 egui 会按「在同一个 Ui 里的第几个滚动区」自动编号，两列恰好都能用，
    // 但换个顺序就会串。id 用常量而不是列头文字：列头是**译文**，切一次语言
    // 滚动位置就全丢了。
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(list_h)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if rows.is_empty() {
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(empty)
                        .color(crate::theme::chrome(CHROME_SCALE.icon))
                        .size(crate::ui::type_scale::SIZE_BODY),
                );
                return;
            }
            for (path, name) in rows {
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), ROW_H),
                    egui::Sense::click(),
                );
                let p = ui.painter();
                if *path == current {
                    p.rect_filled(rect, 0.0, crate::theme::chrome(CHROME_SCALE.selected));
                } else if resp.hovered() {
                    // 悬停与菜单同一支灰：本仓里「指向色」只有一种写法。
                    p.rect_filled(rect, 0.0, crate::theme::chrome(CHROME_SCALE.hover));
                }
                p.text(
                    egui::pos2(rect.left() + 6.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    name,
                    crate::ui::type_scale::body(),
                    crate::theme::chrome(CHROME_SCALE.text),
                );
                let resp = resp.on_hover_text(path);
                if resp.clicked() {
                    picked = Some(path.clone());
                }
            }
        });
    picked
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 只有 `*.rhai` 进清单，且按展示名排序。
    ///
    /// 「把 `notes.rhai.bak` 也列出来」是最容易犯的错：`extension()` 只看最后一个
    /// 点，而那种文件在编辑器里打不开、运行会直接报语法错。
    #[test]
    fn only_rhai_files_reach_the_library_list() {
        let got = collect(vec![
            PathBuf::from("/s/b.rhai"),
            PathBuf::from("/s/a.rhai"),
            PathBuf::from("/s/c.rhai.bak"),
            PathBuf::from("/s/d.txt"),
            PathBuf::from("/s/noext"),
        ]);
        assert_eq!(
            got,
            vec![
                ("/s/a.rhai".to_string(), "a.rhai".to_string()),
                ("/s/b.rhai".to_string(), "b.rhai".to_string()),
            ],
            "只留 .rhai，且按文件名排序"
        );
    }

    /// 展示名取文件名；没有文件名时退回整串（宁可长，不可空）。
    #[test]
    fn a_row_shows_the_file_name_but_keeps_the_whole_path() {
        assert_eq!(display_name("/tmp/x/deploy.rhai"), "deploy.rhai");
        assert_eq!(display_name("deploy.rhai"), "deploy.rhai");
        // `/` 与 `..` 都不是文件，`file_name()` 给 None，不能让整行变成空白。
        assert_eq!(display_name("/"), "/");
        assert_eq!(display_name(".."), "..");
    }

    /// 空清单 = 空表，不是 panic；这条同时守住「库目录不存在」那条路径的形状。
    #[test]
    fn an_empty_library_is_an_empty_list() {
        assert!(collect(Vec::<PathBuf>::new()).is_empty());
    }
}
