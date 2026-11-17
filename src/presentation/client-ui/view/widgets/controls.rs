use egui::class::HasClasses;

use crate::view::theme::{BUTTON_TEXT, DISABLED_BUTTON};

pub(super) const SELECTOR_HEIGHT: f32 = 34.0;

pub(super) fn paint_spinner(ui: &egui::Ui, center: egui::Pos2, size: f32) {
    egui::Spinner::new().paint_at(
        ui,
        egui::Rect::from_center_size(center, egui::Vec2::splat(size)),
    );
}

pub(super) fn selector(
    ui: &mut egui::Ui,
    id: &str,
    width: f32,
    selected: &str,
    contents: impl FnOnce(&mut egui::Ui),
) {
    ui.scope(|ui| {
        let width = width.min(ui.available_width());
        let height = (ui.ctx().content_rect().height() - 48.0).clamp(52.0, 260.0);
        let spacing = ui.spacing_mut();
        spacing.interact_size.y = SELECTOR_HEIGHT;
        spacing.button_padding = egui::vec2(10.0, 4.0);
        let menu_widgets = ui.visuals().widgets.clone();
        let enabled = ui.is_enabled();
        let widgets = &mut ui.visuals_mut().widgets;
        for state in [
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
            &mut widgets.open,
        ] {
            state.weak_bg_fill = state.bg_fill;
            state.expansion = 0.0;
            state.corner_radius = egui::CornerRadius::same(5);
            if enabled {
                state.fg_stroke.color = BUTTON_TEXT;
            }
        }
        widgets.open.weak_bg_fill = widgets.inactive.bg_fill;
        let mut selector = egui::ComboBox::from_id_salt(id)
            .width(width)
            .height(height)
            .selected_text((egui::Atom::grow(), selected))
            .truncate()
            .popup_style(egui::style::StyleModifier::new(move |style| {
                style.spacing.menu_margin = egui::Margin::same(3);
                style.spacing.item_spacing.y = 0.0;
                style.visuals.widgets = menu_widgets.clone();
                style.visuals.window_stroke = egui::Stroke::new(1.0, style.visuals.hyperlink_color);
                style.visuals.window_corner_radius = egui::CornerRadius::same(5);
                style.visuals.popup_shadow = egui::epaint::Shadow::NONE;
                style.visuals.widgets.hovered.bg_stroke =
                    egui::Stroke::new(1.0, style.visuals.hyperlink_color);
                style.visuals.widgets.hovered.expansion = 0.0;
            }));
        selector.add_class_if(DISABLED_BUTTON, !enabled);
        let _response = selector.show_ui(ui, contents).response;
        #[cfg(all(debug_assertions, feature = "demo"))]
        if let Some(target) = ui
            .ctx()
            .data(|data| data.get_temp::<&str>(selector_capture_id()))
            && target == id
        {
            ui.ctx().data_mut(|data| {
                data.insert_temp(
                    selector_capture_id().with("position"),
                    _response.rect.center(),
                );
            });
        }
    });
}

#[cfg(all(debug_assertions, feature = "demo"))]
pub fn selector_capture_id() -> egui::Id {
    egui::Id::unique("preview-selector")
}

pub(super) fn selector_option<'a>(
    ui: &egui::Ui,
    selected: bool,
    label: &'a str,
) -> egui::Button<'a> {
    egui::Button::selectable(selected, label)
        .min_size(egui::vec2(ui.available_width(), 31.0))
        .truncate()
}

pub(super) fn more_button(size: f32) -> egui::Button<'static> {
    let ellipsis = egui::Atom::paint(egui::Vec2::splat(16.0), |ui, args| {
        for x in [-5.0, 0.0, 5.0] {
            ui.painter().circle_filled(
                args.rect.center() + egui::vec2(x, 0.0),
                1.0,
                ui.visuals().text_color(),
            );
        }
    });
    egui::Button::new(ellipsis)
        .min_size(egui::Vec2::splat(size))
        .frame_when_inactive(false)
}

pub(super) fn select_value<T: PartialEq>(ui: &mut egui::Ui, value: &mut T, option: T, label: &str) {
    if ui
        .add(selector_option(ui, *value == option, label))
        .clicked()
    {
        *value = option;
        ui.close();
    }
}

pub(super) fn roomy_input(input: &mut String, width: f32) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(input)
        .desired_width(width)
        .min_size(egui::vec2(width, SELECTOR_HEIGHT))
        .margin(egui::Margin::symmetric(10, 4))
        .align(egui::Align2::LEFT_CENTER)
}

pub(super) fn feedback(ui: &mut egui::Ui, message: &str, dismiss: Option<&str>) -> bool {
    let accent = ui.visuals().warn_fg_color;
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color.lerp_to_gamma(accent, 0.10))
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.label(egui::RichText::new("!").strong().color(accent));
                let width = ui.available_width() - if dismiss.is_some() { 30.0 } else { 0.0 };
                ui.allocate_ui_with_layout(
                    egui::vec2(width.max(1.0), 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_width(width.max(1.0));
                        ui.add(egui::Label::new(message).wrap());
                    },
                );
                dismiss.is_some_and(|label| {
                    ui.add(egui::Button::new("×").frame(false))
                        .on_hover_text(label)
                        .clicked()
                })
            })
            .inner
        })
        .inner
}
