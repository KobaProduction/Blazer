use std::sync::Arc;

use egui::{FallbackRequest, FontData, FontInsert};
use epaint_default_fonts::NOTO_EMOJI_REGULAR;
use skrifa::{FontRef, MetadataProvider};

pub(super) fn install_emoji_fallback(context: &egui::Context) {
    // eframe registers the OS font provider before creating the app.
    context.add_font_provider(Arc::new(emoji_fallback));
}

fn emoji_fallback(request: &FallbackRequest<'_>) -> Option<FontInsert> {
    let character = request.cluster.chars().next()?;
    let font = FontRef::new(NOTO_EMOJI_REGULAR).ok()?;
    font.charmap().map(character)?;
    Some(FontInsert::new(
        "Noto Emoji",
        FontData::from_static(NOTO_EMOJI_REGULAR),
        Vec::new(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_covers_common_announcement_emoji() {
        let family = egui::FontFamily::Proportional;
        for cluster in ["❌", "😀"] {
            assert!(
                emoji_fallback(&FallbackRequest {
                    cluster,
                    family: &family
                })
                .is_some()
            );
        }
    }
}
