const SCROLLBAR_WIDTH: i8 = 6;
const SCROLLBAR_GAP: i8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ViewportLayout {
    Mobile,
    Tablet,
    Desktop,
}

pub(super) fn modal_content_width(ui: &egui::Ui, maximum: f32, reserved: f32) -> f32 {
    (ui.ctx().content_rect().width() - reserved).clamp(1.0, maximum)
}

impl ViewportLayout {
    pub(super) fn from_width(width: f32) -> Self {
        if !width.is_finite() || width < 600.0 {
            Self::Mobile
        } else if width < 900.0 {
            Self::Tablet
        } else {
            Self::Desktop
        }
    }

    pub(super) fn content_inset(self) -> i8 {
        match self {
            Self::Mobile => 10,
            Self::Tablet => 14,
            Self::Desktop => 22,
        }
    }

    pub(super) fn scroll_frame(self) -> egui::Frame {
        let mut margin = egui::Margin::same(self.content_inset());
        margin.right = 0;
        egui::Frame::new().inner_margin(margin)
    }

    fn scroll_content_margin(self) -> egui::Margin {
        egui::Margin {
            right: self.content_inset().max(SCROLLBAR_WIDTH + SCROLLBAR_GAP),
            ..egui::Margin::ZERO
        }
    }

    pub(super) fn scroll_header(self, ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
        egui::Frame::new()
            .inner_margin(self.scroll_content_margin())
            .show(ui, contents);
    }

    pub(super) fn vertical_scroll(
        self,
        ui: &mut egui::Ui,
        id: impl egui::AsIdSalt,
        contents: impl FnOnce(&mut egui::Ui),
    ) {
        ui.scope(|ui| {
            let width = f32::from(SCROLLBAR_WIDTH);
            // The right margin is permanent. The native scrollbar floats over it on overflow.
            ui.spacing_mut().scroll = egui::style::ScrollStyle {
                floating: true,
                bar_width: width,
                floating_width: width,
                dormant_handle_opacity: 0.7,
                active_handle_opacity: 0.85,
                ..egui::style::ScrollStyle::solid()
            };
            egui::ScrollArea::vertical()
                .id_salt(id)
                .min_scrolled_height(0.0)
                .auto_shrink([false, false])
                .content_margin(self.scroll_content_margin())
                .show(ui, contents);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::ViewportLayout::{self, Desktop, Mobile, Tablet};

    #[test]
    fn logical_width_selects_viewport_layout() {
        for (width, layout) in [
            (f32::NAN, Mobile),
            (f32::INFINITY, Mobile),
            (0.0, Mobile),
            (599.9, Mobile),
            (600.0, Tablet),
            (899.9, Tablet),
            (900.0, Desktop),
            (1024.0, Desktop),
        ] {
            assert_eq!(ViewportLayout::from_width(width), layout);
        }
    }

    #[test]
    fn vertical_scroll_reserves_width_and_paints_at_the_right_edge_on_overflow() {
        for width in [360.0, 760.0, 980.0] {
            let layout = ViewportLayout::from_width(width);
            let context = egui::Context::default();
            let bar_color = egui::Color32::from_rgb(111, 222, 99);
            context.all_styles_mut(|style| {
                style.animation_time = 0.0;
                style.visuals.widgets.inactive.bg_fill = bar_color;
            });
            // Check the width on transition frames too, then inspect the settled scrollbar.
            for (height, check_bar) in [
                (80.0, true),
                (600.0, false),
                (600.0, true),
                (80.0, false),
                (80.0, true),
            ] {
                let mut output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 260.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        let right_edge = ui.available_rect_before_wrap().right();
                        let content_edge = right_edge - f32::from(layout.content_inset());
                        layout.scroll_frame().show(ui, |ui| {
                            layout.scroll_header(ui, |ui| {
                                assert_eq!(ui.available_rect_before_wrap().right(), content_edge);
                                ui.label("Servers");
                            });
                            layout.vertical_scroll(ui, "test-scroll", |ui| {
                                assert_eq!(ui.available_rect_before_wrap().right(), content_edge);
                                ui.allocate_space(egui::vec2(ui.available_width(), height));
                            });
                        });
                    },
                );
                output.textures_delta.clear();
                if !check_bar {
                    continue;
                }
                let scrollbar = output.shapes.iter().find_map(|paint| match &paint.shape {
                    egui::Shape::Rect(rect) if rect.fill == bar_color.gamma_multiply(0.7) => {
                        Some(rect.rect)
                    }
                    _ => None,
                });
                assert_eq!(scrollbar.is_some(), height > 260.0);
                if let Some(bar) = scrollbar {
                    assert_eq!(bar.right(), width);
                    assert_eq!(bar.width(), f32::from(super::SCROLLBAR_WIDTH));
                }
            }
        }
    }
}
