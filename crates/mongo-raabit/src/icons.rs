//! Original 16×16 vector icons drawn with the egui painter (no bundled artwork).

use egui::{Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2, pos2, vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Server,
    Database,
    Collection,
    View,
    TimeSeries,
    Bucket,
    User,
    Folder,
    Leaf,
    Play,
    Refresh,
    Plus,
    Minus,
    Edit,
    Copy,
    Trash,
    Search,
    Close,
    ChevronRight,
    ChevronDown,
    ChevronLeft,
    ChevronUp,
    Table,
    Tree,
    Json,
    Pipeline,
    Sql,
    Shell,
    Schema,
    Index,
    Pulse,
    Code,
    Explain,
    Import,
    Export,
    Plug,
    Unplug,
    Check,
    Warning,
    Filter,
}

/// Allocate a 16×16 (or `size`) icon and paint it.
pub fn icon(ui: &mut Ui, icon: Icon, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
    paint(ui.painter(), rect, icon, color);
    response
}

/// Paint `icon` centered in `rect` (designed on a 16 px grid).
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height()) / 16.0;
    let o = rect.center() - vec2(8.0 * s, 8.0 * s);
    let p = |x: f32, y: f32| -> Pos2 { o + vec2(x * s, y * s) };
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(p(x0, y0), p(x1, y1));
    let stroke = Stroke::new(1.2 * s.max(1.0), color);
    let thin = Stroke::new(1.0 * s.max(1.0), color);
    let line = |a: (f32, f32), b: (f32, f32)| {
        painter.line_segment([p(a.0, a.1), p(b.0, b.1)], stroke);
    };
    let poly = |pts: &[(f32, f32)], closed: bool| {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| p(x, y)).collect();
        if closed {
            painter.add(Shape::closed_line(pts, stroke));
        } else {
            painter.add(Shape::line(pts, stroke));
        }
    };
    match icon {
        Icon::Server => {
            painter.rect_stroke(
                r(2.5, 2.5, 13.5, 7.0),
                2.0 * s,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.rect_stroke(
                r(2.5, 9.0, 13.5, 13.5),
                2.0 * s,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.circle_filled(p(5.0, 4.8), 0.9 * s, color);
            painter.circle_filled(p(5.0, 11.2), 0.9 * s, color);
        }
        Icon::Database => {
            let top = r(3.0, 2.0, 13.0, 5.6);
            painter.add(Shape::ellipse_stroke(
                top.center(),
                top.size() / 2.0,
                stroke,
            ));
            line((3.0, 3.8), (3.0, 12.2));
            line((13.0, 3.8), (13.0, 12.2));
            painter.add(Shape::line(
                arc(p(8.0, 8.0), 5.0 * s, 1.8 * s, true),
                stroke,
            ));
            painter.add(Shape::line(
                arc(p(8.0, 12.2), 5.0 * s, 1.8 * s, true),
                stroke,
            ));
        }
        Icon::Collection | Icon::Table => {
            painter.rect_stroke(
                r(2.5, 3.0, 13.5, 13.0),
                2.0 * s,
                stroke,
                egui::StrokeKind::Inside,
            );
            line((2.5, 6.5), (13.5, 6.5));
            line((2.5, 9.8), (13.5, 9.8));
            line((6.5, 6.5), (6.5, 13.0));
        }
        Icon::View => {
            let pts = [
                (1.5, 8.0),
                (4.5, 4.5),
                (8.0, 3.5),
                (11.5, 4.5),
                (14.5, 8.0),
                (11.5, 11.5),
                (8.0, 12.5),
                (4.5, 11.5),
            ];
            poly(&pts, true);
            painter.circle_stroke(p(8.0, 8.0), 2.0 * s, stroke);
        }
        Icon::TimeSeries => {
            painter.circle_stroke(p(8.0, 8.0), 5.5 * s, stroke);
            line((8.0, 4.8), (8.0, 8.0));
            line((8.0, 8.0), (10.5, 9.5));
        }
        Icon::Bucket => {
            poly(&[(3.0, 4.0), (13.0, 4.0), (12.0, 13.5), (4.0, 13.5)], true);
            line((2.5, 4.0), (13.5, 4.0));
            line((5.5, 7.0), (10.5, 7.0));
        }
        Icon::User => {
            painter.circle_stroke(p(8.0, 5.5), 2.6 * s, stroke);
            painter.add(Shape::line(
                arc(p(8.0, 14.0), 5.0 * s, 4.5 * s, false),
                stroke,
            ));
        }
        Icon::Folder => {
            poly(
                &[
                    (2.0, 4.0),
                    (6.0, 4.0),
                    (7.5, 5.5),
                    (14.0, 5.5),
                    (14.0, 13.0),
                    (2.0, 13.0),
                ],
                true,
            );
        }
        Icon::Leaf => {
            let left = [(8.0, 1.8), (5.0, 5.0), (4.4, 9.0), (6.0, 12.0), (8.0, 13.5)];
            let right = [
                (8.0, 1.8),
                (11.0, 5.0),
                (11.6, 9.0),
                (10.0, 12.0),
                (8.0, 13.5),
            ];
            let mut pts: Vec<Pos2> = left.iter().map(|&(x, y)| p(x, y)).collect();
            pts.extend(right.iter().rev().map(|&(x, y)| p(x, y)));
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
            painter.line_segment(
                [p(8.0, 4.0), p(8.0, 15.0)],
                Stroke::new(1.0 * s, color.gamma_multiply(0.6)),
            );
        }
        Icon::Play => {
            painter.add(Shape::convex_polygon(
                vec![p(4.5, 3.0), p(13.0, 8.0), p(4.5, 13.0)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Refresh => {
            let mut pts = Vec::new();
            for i in 0..=20 {
                let a = std::f32::consts::PI * (0.15 + 1.6 * i as f32 / 20.0);
                pts.push(p(8.0, 8.0) + vec2(a.cos(), -a.sin()) * 5.0 * s);
            }
            painter.add(Shape::line(pts, stroke));
            painter.add(Shape::convex_polygon(
                vec![p(12.6, 2.6), p(13.6, 6.6), p(9.6, 6.0)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Plus => {
            line((8.0, 3.0), (8.0, 13.0));
            line((3.0, 8.0), (13.0, 8.0));
        }
        Icon::Minus => line((3.0, 8.0), (13.0, 8.0)),
        Icon::Edit => {
            poly(
                &[
                    (3.0, 13.0),
                    (3.5, 10.0),
                    (11.0, 2.5),
                    (13.5, 5.0),
                    (6.0, 12.5),
                ],
                true,
            );
            line((9.5, 4.0), (12.0, 6.5));
        }
        Icon::Copy => {
            painter.rect_stroke(
                r(5.5, 5.5, 13.5, 13.5),
                1.5 * s,
                stroke,
                egui::StrokeKind::Inside,
            );
            poly(
                &[
                    (10.5, 3.5),
                    (10.5, 2.5),
                    (2.5, 2.5),
                    (2.5, 10.5),
                    (3.5, 10.5),
                ],
                false,
            );
        }
        Icon::Trash => {
            line((2.5, 4.0), (13.5, 4.0));
            poly(&[(6.0, 4.0), (6.5, 2.5), (9.5, 2.5), (10.0, 4.0)], false);
            poly(&[(4.0, 4.0), (4.8, 13.5), (11.2, 13.5), (12.0, 4.0)], false);
            line((6.8, 6.5), (6.8, 11.0));
            line((9.2, 6.5), (9.2, 11.0));
        }
        Icon::Search => {
            painter.circle_stroke(p(7.0, 7.0), 4.2 * s, stroke);
            line((10.2, 10.2), (13.5, 13.5));
        }
        Icon::Close => {
            line((4.0, 4.0), (12.0, 12.0));
            line((12.0, 4.0), (4.0, 12.0));
        }
        Icon::ChevronRight => poly(&[(6.0, 3.5), (10.5, 8.0), (6.0, 12.5)], false),
        Icon::ChevronLeft => poly(&[(10.0, 3.5), (5.5, 8.0), (10.0, 12.5)], false),
        Icon::ChevronDown => poly(&[(3.5, 6.0), (8.0, 10.5), (12.5, 6.0)], false),
        Icon::ChevronUp => poly(&[(3.5, 10.0), (8.0, 5.5), (12.5, 10.0)], false),
        Icon::Tree => {
            line((4.0, 3.0), (4.0, 12.0));
            line((4.0, 7.0), (8.0, 7.0));
            line((4.0, 12.0), (8.0, 12.0));
            painter.rect_stroke(
                r(2.0, 1.5, 6.0, 4.5),
                1.0 * s,
                thin,
                egui::StrokeKind::Inside,
            );
            painter.rect_stroke(
                r(8.5, 5.5, 13.5, 8.5),
                1.0 * s,
                thin,
                egui::StrokeKind::Inside,
            );
            painter.rect_stroke(
                r(8.5, 10.5, 13.5, 13.5),
                1.0 * s,
                thin,
                egui::StrokeKind::Inside,
            );
        }
        Icon::Json | Icon::Code => {
            poly(&[(5.5, 3.0), (2.0, 8.0), (5.5, 13.0)], false);
            poly(&[(10.5, 3.0), (14.0, 8.0), (10.5, 13.0)], false);
            if icon == Icon::Code {
                line((9.0, 3.5), (7.0, 12.5));
            } else {
                painter.circle_filled(p(8.0, 8.0), 1.0 * s, color);
            }
        }
        Icon::Pipeline => {
            for (y, w) in [(3.5, 11.0), (8.0, 8.0), (12.5, 5.0)] {
                let x0 = 8.0 - w / 2.0;
                painter.rect_stroke(
                    r(x0, y - 1.6, x0 + w, y + 1.6),
                    1.0 * s,
                    thin,
                    egui::StrokeKind::Inside,
                );
            }
        }
        Icon::Sql => {
            painter.rect_stroke(
                r(2.0, 2.5, 14.0, 13.5),
                2.0 * s,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.text(
                p(8.0, 8.2),
                egui::Align2::CENTER_CENTER,
                "SQL",
                egui::FontId::proportional(5.5 * s),
                color,
            );
        }
        Icon::Shell => {
            painter.rect_stroke(
                r(1.5, 2.5, 14.5, 13.5),
                2.0 * s,
                stroke,
                egui::StrokeKind::Inside,
            );
            poly(&[(4.5, 6.0), (6.5, 8.0), (4.5, 10.0)], false);
            line((8.0, 10.5), (11.5, 10.5));
        }
        Icon::Schema => {
            for (x, h) in [(3.0, 5.0), (7.0, 9.0), (11.0, 7.0)] {
                painter.rect_filled(r(x, 13.0 - h, x + 2.6, 13.0), 0.5 * s, color);
            }
            line((1.5, 13.8), (14.5, 13.8));
        }
        Icon::Index => {
            painter.add(Shape::convex_polygon(
                vec![
                    p(9.5, 1.5),
                    p(3.5, 9.0),
                    p(7.5, 9.0),
                    p(6.5, 14.5),
                    p(12.5, 7.0),
                    p(8.5, 7.0),
                ],
                Color32::TRANSPARENT,
                stroke,
            ));
        }
        Icon::Pulse => poly(
            &[
                (1.5, 8.5),
                (4.5, 8.5),
                (6.0, 4.0),
                (8.5, 12.5),
                (10.5, 6.5),
                (11.5, 8.5),
                (14.5, 8.5),
            ],
            false,
        ),
        Icon::Explain => {
            painter.circle_stroke(p(8.0, 3.5), 1.8 * s, thin);
            painter.circle_stroke(p(4.0, 12.0), 1.8 * s, thin);
            painter.circle_stroke(p(12.0, 12.0), 1.8 * s, thin);
            line((7.0, 5.0), (4.8, 10.4));
            line((9.0, 5.0), (11.2, 10.4));
        }
        Icon::Import | Icon::Export => {
            poly(
                &[(2.5, 10.0), (2.5, 13.5), (13.5, 13.5), (13.5, 10.0)],
                false,
            );
            if icon == Icon::Import {
                line((8.0, 2.0), (8.0, 10.0));
                poly(&[(5.0, 7.0), (8.0, 10.0), (11.0, 7.0)], false);
            } else {
                line((8.0, 10.0), (8.0, 2.0));
                poly(&[(5.0, 5.0), (8.0, 2.0), (11.0, 5.0)], false);
            }
        }
        Icon::Plug | Icon::Unplug => {
            painter.rect_stroke(
                r(4.5, 5.0, 11.5, 10.0),
                1.5 * s,
                stroke,
                egui::StrokeKind::Inside,
            );
            line((6.5, 5.0), (6.5, 2.0));
            line((9.5, 5.0), (9.5, 2.0));
            line((8.0, 10.0), (8.0, 14.0));
            if icon == Icon::Unplug {
                line((2.0, 14.0), (14.0, 2.0));
            }
        }
        Icon::Check => poly(&[(3.0, 8.5), (6.5, 12.0), (13.0, 4.5)], false),
        Icon::Warning => {
            poly(&[(8.0, 2.0), (14.5, 13.5), (1.5, 13.5)], true);
            line((8.0, 6.0), (8.0, 9.5));
            painter.circle_filled(p(8.0, 11.5), 0.8 * s, color);
        }
        Icon::Filter => poly(
            &[
                (2.0, 3.0),
                (14.0, 3.0),
                (9.5, 8.5),
                (9.5, 13.5),
                (6.5, 12.0),
                (6.5, 8.5),
            ],
            true,
        ),
    }
}

/// Lower (or upper) half of an ellipse as a polyline.
fn arc(center: Pos2, rx: f32, ry: f32, lower: bool) -> Vec<Pos2> {
    (0..=16)
        .map(|i| {
            let t = std::f32::consts::PI * i as f32 / 16.0;
            let y = if lower { t.sin() } else { -t.sin() };
            pos2(center.x - rx * t.cos(), center.y + ry * y)
        })
        .collect()
}
