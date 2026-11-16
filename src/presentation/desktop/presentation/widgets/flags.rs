use eframe::egui;

const ATLAS: &[u8] = include_bytes!("../../assets/flags.png");
const REGION_CODES: &[u8] = include_bytes!("../../assets/flags.txt");
const TILE: egui::Vec2 = egui::vec2(42.0, 32.0);
const COLUMNS: usize = 16;

pub(super) fn server_name(name: &str) -> &str {
    flag_prefix(name)
        .filter(|(region, _)| atlas_index(region).is_some())
        .map(|(_, rest)| rest.trim_start())
        .filter(|rest| !rest.is_empty())
        .unwrap_or(name)
}

pub(super) fn paint_flag(ui: &egui::Ui, rect: egui::Rect, name: &str) -> bool {
    let Some((region, _)) = flag_prefix(name) else {
        return false;
    };
    let Some(index) = atlas_index(&region) else {
        return false;
    };
    let Some(texture) = atlas_texture(ui.ctx()) else {
        return false;
    };
    let offset = egui::vec2((index % COLUMNS) as f32, (index / COLUMNS) as f32);
    let atlas_size = texture.size_vec2();
    let uv = egui::Rect::from_min_max(
        (offset * TILE / atlas_size).to_pos2(),
        ((offset + egui::Vec2::splat(1.0)) * TILE / atlas_size).to_pos2(),
    );
    ui.painter()
        .image(texture.id(), rect, uv, egui::Color32::WHITE);
    true
}

fn atlas_index(region: &[u8; 2]) -> Option<usize> {
    REGION_CODES.chunks_exact(2).position(|code| code == region)
}

fn atlas_texture(context: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::unique("server-flag-atlas");
    if let Some(handle) = context.data(|data| data.get_temp(id)) {
        return Some(handle);
    }
    let pixels = image::load_from_memory_with_format(ATLAS, image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    let size = [pixels.width() as usize, pixels.height() as usize];
    let handle = context.load_texture(
        "server flags",
        egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_raw()),
        egui::TextureOptions::LINEAR,
    );
    context.data_mut(|data| data.insert_temp(id, handle.clone()));
    Some(handle)
}

fn flag_prefix(name: &str) -> Option<([u8; 2], &str)> {
    let mut chars = name.trim_start().chars();
    let indicator = |character: char| {
        let offset = u32::from(character).checked_sub(0x1F1E6)?;
        (offset < 26).then_some(b'A' + offset.min(25) as u8)
    };
    let code = [indicator(chars.next()?)?, indicator(chars.next()?)?];
    Some((code, chars.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn country_prefix_reuses_an_atlas_tile_without_guessing_a_location() {
        assert_eq!(flag_prefix(" 🇩🇪 Frankfurt"), Some((*b"DE", " Frankfurt")));
        assert_eq!(server_name(" 🇩🇪 Frankfurt"), "Frankfurt");
        assert!(REGION_CODES.chunks_exact(2).any(|code| code == b"DE"));
        assert_eq!(flag_prefix("Frankfurt"), None);
        assert_eq!(server_name("🇩🇪"), "🇩🇪");
        assert_eq!(server_name("🇿🇿 Unknown"), "🇿🇿 Unknown");
    }
}
