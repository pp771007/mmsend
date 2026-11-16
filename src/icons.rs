//! Line icons drawn with the painter, so every icon in the app shares one stroke
//! style instead of mixing glyphs from whichever emoji font is installed.

use std::f32::consts::PI;

use eframe::egui::{Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};

/// Icons are designed on a 20×20 grid and scaled to the target rect.
const GRID: f32 = 20.0;
const STROKE_WIDTH: f32 = 1.6;
const CURVE_SEGMENTS: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Search,
    Eye,
    EyeOff,
    Copy,
    Send,
    Logout,
    Close,
    Check,
    Warning,
    Hash,
    Lock,
    Group,
    Chat,
    Download,
    Login,
    Refresh,
    More,
    History,
    Trash,
    Pin,
    /// Pin, filled: the item is pinned.
    Pinned,
}

/// `background` is the color behind the icon; Copy uses it to hide the back sheet.
pub fn paint(painter: &Painter, icon: Icon, rect: Rect, color: Color32, background: Color32) {
    let scale = rect.width().min(rect.height()) / GRID;
    let origin = rect.center() - Vec2::splat(GRID * scale / 2.0);
    let at = |x: f32, y: f32| origin + Vec2::new(x, y) * scale;
    let stroke = Stroke::new(STROKE_WIDTH * scale, color);
    let line = |points: &[(f32, f32)]| {
        painter.add(Shape::line(points.iter().map(|&(x, y)| at(x, y)).collect(), stroke));
    };
    let closed = |points: Vec<Pos2>| {
        painter.add(Shape::closed_line(points, stroke));
    };
    let rounded = |x0: f32, y0: f32, x1: f32, y1: f32, radius: f32, fill: Option<Color32>| {
        let r = Rect::from_min_max(at(x0, y0), at(x1, y1));
        let corner = CornerRadius::same((radius * scale) as u8);
        if let Some(fill) = fill {
            painter.rect_filled(r, corner, fill);
        }
        painter.rect_stroke(r, corner, stroke, StrokeKind::Middle);
    };
    // Points on an ellipse arc, angles in radians (y grows downwards).
    let arc = |cx: f32, cy: f32, rx: f32, ry: f32, from: f32, to: f32| -> Vec<Pos2> {
        (0..=CURVE_SEGMENTS)
            .map(|i| {
                let a = from + (to - from) * i as f32 / CURVE_SEGMENTS as f32;
                at(cx + rx * a.cos(), cy + ry * a.sin())
            })
            .collect()
    };
    let eye = || {
        let mut outline = arc(10.0, 13.5, 8.2, 8.0, PI * 1.08, PI * 1.92);
        outline.extend(arc(10.0, 6.5, 8.2, 8.0, PI * 0.08, PI * 0.92));
        closed(outline);
        painter.circle_stroke(at(10.0, 10.0), 2.5 * scale, stroke);
    };

    match icon {
        Icon::Search => {
            painter.circle_stroke(at(8.5, 8.5), 5.5 * scale, stroke);
            line(&[(12.6, 12.6), (17.0, 17.0)]);
        }
        Icon::Eye => eye(),
        Icon::EyeOff => {
            eye();
            line(&[(3.5, 3.5), (16.5, 16.5)]);
        }
        Icon::Copy => {
            rounded(7.0, 3.0, 17.0, 13.0, 2.0, None);
            rounded(3.0, 7.0, 13.0, 17.0, 2.0, Some(background));
        }
        Icon::Send => {
            closed(vec![at(2.5, 9.2), at(17.5, 2.5), at(10.8, 17.5), at(8.6, 11.4)]);
            line(&[(8.6, 11.4), (17.5, 2.5)]);
        }
        Icon::Logout => {
            line(&[(8.5, 3.5), (4.5, 3.5), (4.5, 16.5), (8.5, 16.5)]);
            line(&[(8.5, 10.0), (16.5, 10.0)]);
            line(&[(13.0, 6.5), (16.5, 10.0), (13.0, 13.5)]);
        }
        Icon::Close => {
            line(&[(5.0, 5.0), (15.0, 15.0)]);
            line(&[(15.0, 5.0), (5.0, 15.0)]);
        }
        Icon::Check => line(&[(4.0, 10.5), (8.0, 14.5), (16.0, 6.0)]),
        Icon::Warning => {
            closed(vec![at(10.0, 2.8), at(18.0, 16.8), at(2.0, 16.8)]);
            line(&[(10.0, 7.8), (10.0, 11.6)]);
            painter.circle_filled(at(10.0, 14.2), 1.0 * scale, color);
        }
        Icon::Hash => {
            line(&[(8.0, 3.0), (6.5, 17.0)]);
            line(&[(13.5, 3.0), (12.0, 17.0)]);
            line(&[(3.5, 7.5), (16.5, 7.5)]);
            line(&[(3.0, 12.5), (16.0, 12.5)]);
        }
        Icon::Lock => {
            rounded(4.5, 9.0, 15.5, 17.0, 2.0, None);
            let mut shackle = vec![at(7.0, 9.0)];
            shackle.extend(arc(10.0, 6.5, 3.0, 3.0, PI, PI * 2.0));
            shackle.push(at(13.0, 9.0));
            painter.add(Shape::line(shackle, stroke));
        }
        Icon::Group => {
            painter.circle_stroke(at(7.5, 7.0), 2.8 * scale, stroke);
            painter.add(Shape::line(arc(7.5, 17.0, 5.0, 5.0, PI, PI * 2.0), stroke));
            painter.circle_stroke(at(14.0, 6.0), 2.2 * scale, stroke);
            painter.add(Shape::line(arc(14.0, 15.0, 4.0, 4.5, PI * 1.35, PI * 2.0), stroke));
        }
        Icon::Chat => {
            rounded(3.0, 3.5, 17.0, 13.5, 3.0, None);
            line(&[(6.5, 13.5), (5.5, 17.0), (10.0, 13.5)]);
        }
        Icon::Download => {
            line(&[(10.0, 3.0), (10.0, 12.5)]);
            line(&[(6.0, 8.5), (10.0, 12.5), (14.0, 8.5)]);
            line(&[(3.5, 13.5), (3.5, 16.5), (16.5, 16.5), (16.5, 13.5)]);
        }
        Icon::Login => {
            line(&[(11.5, 3.5), (15.5, 3.5), (15.5, 16.5), (11.5, 16.5)]);
            line(&[(3.5, 10.0), (11.5, 10.0)]);
            line(&[(8.0, 6.5), (11.5, 10.0), (8.0, 13.5)]);
        }
        Icon::More => {
            for x in [4.5, 10.0, 15.5] {
                painter.circle_filled(at(x, 10.0), 1.5 * scale, color);
            }
        }
        Icon::History => {
            painter.circle_stroke(at(10.0, 10.0), 7.0 * scale, stroke);
            line(&[(10.0, 5.5), (10.0, 10.0), (13.2, 12.0)]);
        }
        Icon::Trash => {
            line(&[(3.5, 5.5), (16.5, 5.5)]);
            line(&[(8.0, 5.5), (8.0, 3.2), (12.0, 3.2), (12.0, 5.5)]);
            line(&[(5.2, 5.5), (6.2, 17.0), (13.8, 17.0), (14.8, 5.5)]);
            line(&[(8.6, 8.5), (8.9, 14.0)]);
            line(&[(11.4, 8.5), (11.1, 14.0)]);
        }
        Icon::Pin | Icon::Pinned => {
            let body = vec![at(7.0, 3.0), at(13.0, 3.0), at(12.5, 8.5), at(15.5, 12.0), at(4.5, 12.0), at(7.5, 8.5)];
            if icon == Icon::Pinned {
                painter.add(Shape::convex_polygon(body.clone(), color, Stroke::NONE));
            }
            closed(body);
            line(&[(10.0, 12.0), (10.0, 17.5)]);
        }
        Icon::Refresh => {
            let (from, to) = (-0.4 * PI, 1.3 * PI);
            painter.add(Shape::line(arc(10.0, 10.0, 6.0, 6.0, from, to), stroke));
            // Arrowhead at the end of the arc, pointing along its direction of travel.
            let end = (10.0 + 6.0 * to.cos(), 10.0 + 6.0 * to.sin());
            let along = (-to.sin(), to.cos());
            let across = (-along.1, along.0);
            let head = vec![
                at(end.0 + along.0 * 2.8, end.1 + along.1 * 2.8),
                at(end.0 + across.0 * 2.6, end.1 + across.1 * 2.6),
                at(end.0 - across.0 * 2.6, end.1 - across.1 * 2.6),
            ];
            painter.add(Shape::convex_polygon(head, color, Stroke::NONE));
        }
    }
}
