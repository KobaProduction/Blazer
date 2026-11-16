use eframe::egui::{self, Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind};

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Expand,
    Collapse,
    Add,
    Servers,
    Settings,
    Statistics,
    Logs,
    About,
    Link,
}

impl Icon {
    pub(super) fn paint(self, painter: &Painter, center: Pos2, color: Color32) {
        let stroke = Stroke::new(1.4, color);
        match self {
            Self::Expand => {
                painter.arrow(center - egui::vec2(7.0, 0.0), egui::vec2(14.0, 0.0), stroke)
            }
            Self::Collapse => painter.arrow(
                center + egui::vec2(7.0, 0.0),
                egui::vec2(-14.0, 0.0),
                stroke,
            ),
            Self::Add => paint_add(painter, center, stroke),
            Self::Servers => paint_servers(painter, center, stroke),
            Self::Settings => paint_settings(painter, center, stroke),
            Self::Statistics => paint_statistics(painter, center, stroke),
            Self::Logs => paint_logs(painter, center, stroke),
            Self::About => paint_info(painter, center, color),
            Self::Link => paint_link(painter, center, stroke),
        }
    }
}

pub(super) fn paint_failure_badge(painter: &Painter, center: Pos2, fill: Color32, mark: Color32) {
    painter.circle_filled(center, 9.0, fill);
    let stroke = Stroke::new(1.7, mark);
    for y in [-3.25, 3.25] {
        let offset = egui::vec2(3.25, y);
        painter.line_segment([center - offset, center + offset], stroke);
    }
}

fn paint_link(painter: &Painter, center: Pos2, stroke: Stroke) {
    let loop_points = [
        (-1.0, -4.0),
        (-5.0, -4.0),
        (-7.5, -2.0),
        (-7.5, 2.0),
        (-5.0, 4.0),
        (-2.0, 4.0),
        (1.0, 1.0),
    ];
    for direction in [-1.0, 1.0] {
        painter.add(Shape::line(
            loop_points
                .iter()
                .map(|&(x, y)| center + egui::vec2(x, y) * direction)
                .collect(),
            stroke,
        ));
    }
}

fn paint_add(painter: &Painter, center: Pos2, stroke: Stroke) {
    let bounds = Rect::from_center_size(center, egui::Vec2::splat(16.0));
    painter.rect_stroke(bounds, 4, stroke, StrokeKind::Inside);
    painter.hline(center.x - 4.0..=center.x + 4.0, center.y, stroke);
    painter.vline(center.x, center.y - 4.0..=center.y + 4.0, stroke);
}

fn paint_servers(painter: &Painter, center: Pos2, stroke: Stroke) {
    painter.circle_stroke(center, 8.0, stroke);
    painter.add(Shape::ellipse_stroke(center, egui::vec2(4.0, 8.0), stroke));
    painter.hline(center.x - 8.0..=center.x + 8.0, center.y, stroke);
}

fn paint_settings(painter: &Painter, center: Pos2, stroke: Stroke) {
    painter.circle_stroke(center, 3.2, stroke);
    painter.circle_stroke(center, 6.5, stroke);
    for tooth in 0..8 {
        let angle = tooth as f32 * std::f32::consts::TAU / 8.0;
        let axis = egui::vec2(angle.cos(), angle.sin());
        painter.line_segment([center + axis * 6.5, center + axis * 8.5], stroke);
    }
}

fn paint_statistics(painter: &Painter, center: Pos2, stroke: Stroke) {
    painter.rect_stroke(
        Rect::from_center_size(center, egui::Vec2::splat(16.0)),
        4,
        stroke,
        StrokeKind::Inside,
    );
    painter.add(Shape::line(
        [
            (-6.0, 2.0),
            (-3.0, 2.0),
            (0.0, -3.0),
            (3.0, 2.0),
            (6.0, 2.0),
        ]
        .map(|(x, y)| center + egui::vec2(x, y))
        .to_vec(),
        stroke,
    ));
}

fn paint_logs(painter: &Painter, center: Pos2, stroke: Stroke) {
    painter.rect_stroke(
        Rect::from_center_size(center, egui::vec2(13.0, 16.0)),
        3,
        stroke,
        StrokeKind::Inside,
    );
    for offset in [-3.5, 0.0, 3.5] {
        painter.hline(center.x - 3.1..=center.x + 3.1, center.y + offset, stroke);
    }
}

pub(crate) fn paint_info(painter: &Painter, center: Pos2, color: Color32) {
    let stroke = Stroke::new(1.4, color);
    painter.circle_stroke(center, 7.5, stroke);
    painter.circle_filled(center - egui::vec2(0.0, 3.0), 0.9, color);
    painter.vline(center.x, center.y - 0.8..=center.y + 3.8, stroke);
}
