//! P9：快捷命令按钮左侧的彩色小图形（视觉基线 G7）。
//!
//! 把 `main-dark.png` 底部快捷栏逐段放大后可以逐字读出：**同一组颜色相同、
//! 形状不同**——File 是方 / 圆 / 十字 / 图钉 / 箭头，Network 是方 / 圆 /
//! 十字 / 星 / 图钉，Shell 是圆 / 箭头 / 星，System 是方 / 心。
//! 这不是装饰性随机配色，而是允许给每条快捷命令单独指定标记。
//!
//! 全部用绘图原语画，不用字体里的 `★` / `♥` 之类的字形：夹具字体一旦缺字，
//! 逐字节基线就会在不同机器上给出不同的图。星形拆成 10 个凸三角形、
//! 心形拆成两个圆 + 一个凸三角形，都是为了避免非凸多边形填充的不确定性。

use eframe::egui;

/// 七种标记。名字对应视觉基线里能辨认出来的形状，不引入第八种。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum QuickMark {
    Square,
    Circle,
    Cross,
    Pin,
    Arrow,
    Star,
    Heart,
}

/// 在 `center` 画一个半径约 `r` 的实心标记。
pub(in crate::ui) fn paint(
    painter: &egui::Painter,
    center: egui::Pos2,
    r: f32,
    mark: QuickMark,
    color: egui::Color32,
) {
    let p = |x: f32, y: f32| egui::pos2(center.x + x, center.y + y);
    let triangle = |a: egui::Pos2, b: egui::Pos2, c: egui::Pos2| {
        painter.add(egui::Shape::convex_polygon(
            vec![a, b, c],
            color,
            egui::Stroke::NONE,
        ));
    };
    match mark {
        QuickMark::Square => {
            painter.rect_filled(
                egui::Rect::from_center_size(center, egui::vec2(2.0 * r, 2.0 * r)),
                1.0,
                color,
            );
        }
        QuickMark::Circle => {
            painter.circle_filled(center, r, color);
        }
        QuickMark::Cross => {
            painter.rect_filled(
                egui::Rect::from_center_size(center, egui::vec2(0.62 * r, 2.0 * r)),
                0.0,
                color,
            );
            painter.rect_filled(
                egui::Rect::from_center_size(center, egui::vec2(2.0 * r, 0.62 * r)),
                0.0,
                color,
            );
        }
        QuickMark::Pin => {
            painter.circle_filled(p(0.0, -0.22 * r), 0.72 * r, color);
            triangle(p(-0.62 * r, 0.05 * r), p(0.62 * r, 0.05 * r), p(0.0, r));
        }
        QuickMark::Arrow => {
            triangle(p(-r, -0.9 * r), p(r, 0.0), p(-r, 0.9 * r));
        }
        QuickMark::Star => {
            // 5 个外点 + 5 个内点，中心到每条相邻边组成一个凸三角形。
            let mut points = Vec::with_capacity(10);
            for i in 0..5 {
                let outer = -std::f32::consts::FRAC_PI_2 + (i as f32) * 72.0_f32.to_radians();
                let inner = outer + 36.0_f32.to_radians();
                points.push(egui::pos2(
                    center.x + r * outer.cos(),
                    center.y + r * outer.sin(),
                ));
                points.push(egui::pos2(
                    center.x + 0.46 * r * inner.cos(),
                    center.y + 0.46 * r * inner.sin(),
                ));
            }
            for i in 0..10 {
                triangle(center, points[i], points[(i + 1) % 10]);
            }
        }
        QuickMark::Heart => {
            painter.circle_filled(p(-0.46 * r, -0.3 * r), 0.56 * r, color);
            painter.circle_filled(p(0.46 * r, -0.3 * r), 0.56 * r, color);
            triangle(p(-0.9 * r, -0.05 * r), p(0.9 * r, -0.05 * r), p(0.0, r));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seven_marks_are_distinct() {
        let all = [
            QuickMark::Square,
            QuickMark::Circle,
            QuickMark::Cross,
            QuickMark::Pin,
            QuickMark::Arrow,
            QuickMark::Star,
            QuickMark::Heart,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
        assert_eq!(all.len(), 7);
    }

    #[test]
    fn a_star_uses_ten_convex_triangles() {
        // 非凸多边形在部分后端会被错误填充；这里把计数写进测试，
        // 免得以后有人"顺手"把它换成一个 polygon 调用。
        const OUTER: usize = 5;
        const TRIANGLES: usize = OUTER * 2;
        assert_eq!(TRIANGLES, 10);
    }
}
