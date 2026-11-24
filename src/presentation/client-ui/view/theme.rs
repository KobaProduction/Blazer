use egui::class::{ClassName, HasClasses};
use egui::theme::{DefaultStyle, StyleProvider};
use egui::widget_style::{ButtonStyle, StyleArgs, WidgetState};
use egui::{Button, Color32, ComboBox, Theme, Visuals};

const ACCENT: Color32 = Color32::from_rgb(255, 145, 77);
const BUTTON_HOVER: Color32 = Color32::from_rgb(255, 171, 115);
const BUTTON_PRESSED: Color32 = Color32::from_rgb(230, 118, 56);
pub(super) const BUTTON_TEXT: Color32 = Color32::from_gray(20);
pub(super) const DISABLED_BUTTON: ClassName = ClassName::from_static("blazer::disabled_button");
pub(super) const SECONDARY_BUTTON: ClassName = ClassName::from_static("blazer::secondary_button");
pub(super) const DESTRUCTIVE_BUTTON: ClassName =
    ClassName::from_static("blazer::destructive_button");

struct Palette {
    background: Color32,
    surface: Color32,
    inset: Color32,
    control: Color32,
    hover: Color32,
    pressed: Color32,
    selected: Color32,
    border: Color32,
    text: Color32,
    muted: Color32,
    accent: Color32,
    success: Color32,
    ping_fair: Color32,
    ping_poor: Color32,
}

const DARK: Palette = Palette {
    background: Color32::from_gray(17),
    surface: Color32::from_gray(27),
    inset: Color32::from_gray(13),
    control: Color32::from_gray(42),
    hover: Color32::from_gray(53),
    pressed: Color32::from_gray(62),
    selected: Color32::from_gray(40),
    border: Color32::from_gray(58),
    text: Color32::from_gray(209),
    muted: Color32::from_gray(162),
    accent: ACCENT,
    success: Color32::from_rgb(95, 180, 146),
    ping_fair: Color32::from_rgb(240, 190, 84),
    ping_poor: Color32::from_rgb(244, 119, 119),
};

const LIGHT: Palette = Palette {
    background: Color32::from_gray(248),
    surface: Color32::from_gray(255),
    inset: Color32::from_gray(242),
    control: Color32::from_gray(231),
    hover: Color32::from_gray(222),
    pressed: Color32::from_gray(209),
    selected: Color32::from_rgb(255, 235, 220),
    border: Color32::from_gray(192),
    text: Color32::from_gray(42),
    muted: Color32::from_gray(90),
    accent: Color32::from_rgb(139, 46, 17),
    success: Color32::from_rgb(28, 110, 74),
    ping_fair: Color32::from_rgb(141, 87, 0),
    ping_poor: Color32::from_rgb(173, 41, 41),
};

pub(super) fn apply(context: &egui::Context) {
    context.set_visuals_of(Theme::Dark, DARK.visuals(Visuals::dark()));
    context.set_visuals_of(Theme::Light, LIGHT.visuals(Visuals::light()));
    context.replace_widget_theme::<ButtonStyle>(AccentButtons);
}

struct AccentButtons;

impl StyleProvider<ButtonStyle> for AccentButtons {
    fn style(&mut self, args: &StyleArgs<'_>) -> ButtonStyle {
        let mut button: ButtonStyle = DefaultStyle.style(args);
        let classes = args.classes;
        if classes.has_class(&DESTRUCTIVE_BUTTON) && args.state != WidgetState::Noninteractive {
            return destructive_style(button, args);
        }
        if classes.has_class(&SECONDARY_BUTTON)
            || button.atom_layout.frame.fill == Color32::TRANSPARENT
            || classes.has_class(&Button::CLASS_HIDE_FRAME_WHEN_INACTIVE)
            || classes.has_class(&Button::CLASS_SELECTED)
            || classes.has_class(&DISABLED_BUTTON)
            || args.state == WidgetState::Noninteractive
        {
            return button;
        }
        let fill = match args.state {
            WidgetState::Active => BUTTON_PRESSED,
            _ if classes.has_class(&ComboBox::CLASS_OPEN) => BUTTON_PRESSED,
            WidgetState::Hovered => BUTTON_HOVER,
            _ => ACCENT,
        };
        button.atom_layout.frame.fill = fill;
        button.atom_layout.text_style.color = BUTTON_TEXT;
        button
    }
}

fn destructive_style(mut button: ButtonStyle, args: &StyleArgs<'_>) -> ButtonStyle {
    let color = ping_failure_color(&args.style.visuals);
    let strength = match args.state {
        WidgetState::Active => 0.24,
        WidgetState::Hovered => 0.18,
        _ => 0.10,
    };
    button.atom_layout.frame.fill = color.gamma_multiply(strength);
    button.atom_layout.text_style.color = color;
    button
}

pub(super) fn success_color(visuals: &Visuals) -> Color32 {
    if visuals.dark_mode {
        DARK.success
    } else {
        LIGHT.success
    }
}

pub(super) fn ping_color(visuals: &Visuals, milliseconds: u64) -> Color32 {
    let palette = if visuals.dark_mode { &DARK } else { &LIGHT };
    match milliseconds {
        0..=150 => palette.success,
        151..=300 => palette.ping_fair,
        _ => palette.ping_poor,
    }
}

pub(super) fn ping_failure_color(visuals: &Visuals) -> Color32 {
    if visuals.dark_mode {
        DARK.ping_poor
    } else {
        LIGHT.ping_poor
    }
}

impl Palette {
    fn visuals(&self, mut visuals: Visuals) -> Visuals {
        visuals.panel_fill = self.background;
        visuals.window_fill = self.surface;
        visuals.window_stroke.color = self.border;
        visuals.faint_bg_color = self.surface;
        visuals.extreme_bg_color = self.inset;
        visuals.code_bg_color = self.inset;
        visuals.widgets.noninteractive.bg_fill = self.surface;
        visuals.widgets.noninteractive.weak_bg_fill = self.surface;
        visuals.widgets.noninteractive.bg_stroke.color = self.border;
        visuals.widgets.noninteractive.fg_stroke.color = self.text;
        for (state, fill) in [
            (&mut visuals.widgets.inactive, self.control),
            (&mut visuals.widgets.hovered, self.hover),
            (&mut visuals.widgets.active, self.pressed),
            (&mut visuals.widgets.open, self.hover),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
        }
        visuals.widgets.hovered.bg_stroke.color = self.border;
        visuals.widgets.open.bg_stroke.color = self.border;
        visuals.widgets.active.bg_stroke.color = self.accent;
        visuals.selection.bg_fill = self.selected;
        visuals.selection.stroke.color = self.accent;
        visuals.hyperlink_color = self.accent;
        visuals.weak_text_color = Some(self.muted);
        visuals
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_colors_change_after_150_and_300_milliseconds() {
        for (visuals, palette) in [(Visuals::dark(), &DARK), (Visuals::light(), &LIGHT)] {
            assert_eq!(ping_color(&visuals, 150), palette.success);
            assert_eq!(ping_color(&visuals, 151), palette.ping_fair);
            assert_eq!(ping_color(&visuals, 300), palette.ping_fair);
            assert_eq!(ping_color(&visuals, 301), palette.ping_poor);
        }
    }
}
