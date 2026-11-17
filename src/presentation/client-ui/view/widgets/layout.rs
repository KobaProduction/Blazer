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
}
