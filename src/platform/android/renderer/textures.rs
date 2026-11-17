use std::collections::BTreeMap;
use std::sync::Arc;

use egui::epaint::ImageDelta;
use egui::{ColorImage, ImageData, TextureId, TexturesDelta};

// Keep the texture data needed to repopulate GLES after a lost context.
const MAX_SHADOW_PIXELS: usize = 24 * 1024 * 1024 / 4;

#[derive(Default)]
pub(super) struct Textures {
    full: BTreeMap<TextureId, ImageDelta>,
    pixels: usize,
    replay: bool,
}

impl Textures {
    pub(super) fn request_replay(&mut self) {
        self.replay = true;
    }

    pub(super) fn prepare(&mut self, delta: &mut TexturesDelta) -> Option<()> {
        for (id, changes) in &delta.set {
            for change in changes {
                self.apply(*id, change)?;
            }
        }
        if std::mem::take(&mut self.replay) {
            delta.set.clear();
            for (id, image) in &self.full {
                delta.push(*id, image.clone());
            }
        }
        for id in &delta.free {
            if let Some(image) = self.full.remove(id) {
                self.pixels -= pixels(&image);
            }
        }
        Some(())
    }

    fn apply(&mut self, id: TextureId, change: &ImageDelta) -> Option<()> {
        if let Some(offset) = change.pos {
            let target = self.full.get_mut(&id)?;
            let (ImageData::Color(target_image), ImageData::Color(patch)) =
                (&mut target.image, &change.image);
            patch_image(Arc::make_mut(target_image), patch, offset)?;
            target.options = change.options;
        } else {
            let replacing = self.full.get(&id).map_or(0, pixels);
            let total = (self.pixels - replacing).checked_add(pixels(change))?;
            if total > MAX_SHADOW_PIXELS {
                return None;
            }
            self.full.insert(id, change.clone());
            self.pixels = total;
        }
        Some(())
    }
}

fn pixels(image: &ImageDelta) -> usize {
    let ImageData::Color(image) = &image.image;
    image.pixels.len()
}

fn patch_image(target: &mut ColorImage, patch: &ColorImage, [x, y]: [usize; 2]) -> Option<()> {
    let [width, height] = patch.size;
    let [row_width, row_count] = target.size;
    if width == 0 || x.checked_add(width)? > row_width || y.checked_add(height)? > row_count {
        return None;
    }
    for (index, row) in patch.pixels.chunks_exact(width).enumerate() {
        let start = (y + index) * row_width + x;
        target.pixels[start..start + width].copy_from_slice(row);
    }
    Some(())
}
