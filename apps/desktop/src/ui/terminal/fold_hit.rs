//! 折叠列的点击命中：**只看指针几何**，不借内容区的 `Response`。
//!
//! margin 的右键 interact 注册在内容 `resp` 之后，重叠区被它抢走 hit-test，
//! 于是 `resp.clicked()` / `resp.interact_pointer_pos()` 在折叠列上恒不命中——
//! 表现是「点 ▸ 没反应」。这里从 `ui.input` 取本次点击的坐标，直接按
//! `MarginMetrics` 给出的那一格与屏幕行算命中，判据与 `margin::paint` 画 `▸`
//! 的那一格同源。

use super::margin::fold_cmd_for_modifiers;
use term_app::folds::{block_at, Folds};
use term_core::coords::HistoryRow;
use term_core::grid::ScreenImage;

/// 指针这次点击落在折叠列的哪个绝对行上；不在本列 / 没有点击 → `None`。
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn fold_click_at(
    fold_on: bool,
    fold_center: f32,
    cell_w: f32,
    area_min: egui::Pos2,
    geom_start: egui::Pos2,
    geom_cell: egui::Vec2,
    image: &ScreenImage,
    folds: &Folds,
    inputs: &[HistoryRow],
    click_at: Option<egui::Pos2>,
    mods: egui::Modifiers,
) -> Option<term_app::vocab::FoldCmd> {
    if !fold_on {
        return None;
    }
    let p = click_at?;
    let x0 = area_min.x + fold_center - cell_w * 0.5;
    let x1 = area_min.x + fold_center + cell_w * 0.5;
    if !(x0..=x1).contains(&p.x) {
        return None;
    }
    let row = ((p.y - geom_start.y) / geom_cell.y).floor().max(0.0) as usize;
    let line = image.lines.get(row)?;
    let abs = HistoryRow::new(image.history as i64 + line.row)?;
    if folds.regions().iter().any(|r| r.start() == abs) || block_at(inputs, abs).is_some() {
        Some(fold_cmd_for_modifiers(mods, folds.is_empty(), abs))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> ScreenImage {
        let mut img = ScreenImage::blank(20, 1, 0..3);
        img.history = 100;
        img
    }

    fn metrics() -> (bool, f32, f32, egui::Pos2, egui::Pos2, egui::Vec2) {
        // 折叠列中心在 x=15，宽 10；正文从 x=20 起、第一行顶 y=8。
        (true, 15.0, 10.0, egui::pos2(0.0, 0.0), egui::pos2(20.0, 8.0), egui::vec2(10.0, 20.0))
    }

    fn fold_at(abs: i64) -> Folds {
        let mut folds = Folds::new();
        let start = HistoryRow::new(abs).expect("夹具行号非负");
        let end = HistoryRow::new(abs + 2).expect("夹具行号非负");
        let _ = folds.add(term_app::folds::FoldRegion::new(start, end).expect("夹具区间合法"));
        folds
    }

    #[test]
    fn a_click_on_the_fold_column_hits_its_absolute_head_row() {
        let (on, centre, cw, amin, gstart, cell) = metrics();
        let folds = fold_at(101); // 第二行（image.line[1].row == 1 → abs 101）
        let click = Some(egui::pos2(15.0, 28.0));
        let cmd = fold_click_at(on, centre, cw, amin, gstart, cell, &image(), &folds, &[], click, egui::Modifiers::NONE);
        assert_eq!(cmd, Some(term_app::vocab::FoldCmd::ToggleAt(HistoryRow::new(101).expect("夹具行号非负"))));
    }

    #[test]
    fn a_click_outside_the_column_or_without_a_pointer_is_ignored() {
        let (on, centre, cw, amin, gstart, cell) = metrics();
        let folds = fold_at(101);
        let img = image();
        // 正文里点、列没开、没有本帧点击：三条都不得误命中。
        for click in [Some(egui::pos2(25.0, 28.0)), Some(egui::pos2(5.0, 28.0)), None] {
            assert_eq!(fold_click_at(on, centre, cw, amin, gstart, cell, &img, &folds, &[], click, egui::Modifiers::NONE), None);
        }
        assert_eq!(fold_click_at(false, centre, cw, amin, gstart, cell, &img, &folds, &[], Some(egui::pos2(15.0, 28.0)), egui::Modifiers::NONE), None);
    }

    #[test]
    fn a_row_without_a_fold_head_is_not_clickable() {
        let (on, centre, cw, amin, gstart, cell) = metrics();
        // folds 的头在 101，点的是第一行（abs 100）：不该产生折叠命令。
        let folds = fold_at(101);
        assert_eq!(fold_click_at(on, centre, cw, amin, gstart, cell, &image(), &folds, &[], Some(egui::pos2(15.0, 18.0)), egui::Modifiers::NONE), None);
    }
}
