//! EGR1 wire format for the Android GLES host. Fields are little-endian.

use egui::epaint::{ImageDelta, Mesh16, Primitive};
use egui::{FullOutput, OutputCommand, Pos2, Rect, TextureId, ViewportCommand, ViewportId};

use super::Frame;

const MAX_PACKET_BYTES: usize = 32 * 1024 * 1024;
const MESH_COUNT_OFFSET: usize = 8;

pub(super) fn encode(
    context: &egui::Context,
    mut output: FullOutput,
    frame: Frame,
    keyboard_paste: bool,
) -> Option<Vec<u8>> {
    let texture_sets = std::mem::take(&mut output.textures_delta.set);
    let texture_frees = std::mem::take(&mut output.textures_delta.free);
    let ui = UiOutput::new(&output, keyboard_paste);
    let mut packet = Packet(b"EGR1".to_vec());
    packet.length(texture_sets.values().map(|changes| changes.len()).sum())?;
    packet.u32(0); // Patched with the count of emitted u16 meshes.
    packet.length(texture_frees.len())?;
    packet.u32(ui.flags);
    packet.string(ui.copy)?;
    packet.string(ui.url)?;
    for (texture, changes) in texture_sets {
        for change in &changes {
            packet.texture(texture, change)?;
        }
    }
    let mut meshes = 0_u32;
    let scale = output.pixels_per_point;
    let bounds = Rect::from_min_size(Pos2::ZERO, frame.size_pixels);
    for clipped in context.tessellate(output.shapes, scale) {
        let Primitive::Mesh(mesh) = clipped.primitive else {
            continue;
        };
        let clip = (clipped.clip_rect * scale).intersect(bounds);
        if !clip.is_positive() || mesh.indices.is_empty() {
            continue;
        }
        for mesh in mesh.split_to_u16() {
            packet.mesh(&mesh, clip, scale)?;
            meshes = meshes.checked_add(1)?;
        }
    }
    packet.0[MESH_COUNT_OFFSET..MESH_COUNT_OFFSET + 4].copy_from_slice(&meshes.to_le_bytes());
    packet.room(texture_frees.len().checked_mul(8)?)?;
    for texture in texture_frees {
        packet.u64(managed(texture)?);
    }
    Some(packet.0)
}

struct UiOutput<'a> {
    flags: u32,
    copy: &'a str,
    url: &'a str,
}

impl<'a> UiOutput<'a> {
    fn new(output: &'a FullOutput, keyboard_paste: bool) -> Self {
        let paste = keyboard_paste
            || output
                .viewport_output
                .get(&ViewportId::ROOT)
                .is_some_and(|viewport| {
                    viewport
                        .commands
                        .iter()
                        .any(|command| matches!(command, ViewportCommand::RequestPaste))
                });
        let mut result = Self {
            flags: u32::from(output.platform_output.ime.is_some()) | (u32::from(paste) << 1),
            copy: "",
            url: "",
        };
        for command in &output.platform_output.commands {
            match command {
                OutputCommand::CopyText(text) => result.copy = text,
                OutputCommand::OpenUrl(url) => result.url = &url.url,
                _ => {}
            }
        }
        result
    }
}

struct Packet(Vec<u8>);

impl Packet {
    fn room(&self, bytes: usize) -> Option<()> {
        (self.0.len().checked_add(bytes)? <= MAX_PACKET_BYTES).then_some(())
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn f32(&mut self, value: f32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn length(&mut self, value: usize) -> Option<()> {
        self.u32(u32::try_from(value).ok()?);
        Some(())
    }

    fn string(&mut self, value: &str) -> Option<()> {
        self.room(value.len().checked_add(4)?)?;
        self.length(value.len())?;
        self.0.extend_from_slice(value.as_bytes());
        Some(())
    }

    fn texture(&mut self, texture: TextureId, delta: &ImageDelta) -> Option<()> {
        let egui::ImageData::Color(image) = &delta.image;
        let [width, height] = image.size;
        self.room(image.pixels.len().checked_mul(4)?.checked_add(24)?)?;
        self.u64(managed(texture)?);
        let offset = match delta.pos {
            Some([x, y]) => [i32::try_from(x).ok()?, i32::try_from(y).ok()?],
            None => [-1, -1],
        };
        for value in offset {
            self.0.extend_from_slice(&value.to_le_bytes());
        }
        self.length(width)?;
        self.length(height)?;
        for pixel in &image.pixels {
            self.0.extend_from_slice(&pixel.to_array());
        }
        Some(())
    }

    fn mesh(&mut self, mesh: &Mesh16, clip: Rect, scale: f32) -> Option<()> {
        let vertices = mesh.vertices.len().checked_mul(20)?;
        let indices = mesh.indices.len().checked_mul(2)?;
        self.room(vertices.checked_add(indices)?.checked_add(32)?)?;
        self.u64(managed(mesh.texture_id)?);
        for value in [clip.min.x, clip.min.y, clip.max.x, clip.max.y] {
            self.f32(value);
        }
        self.length(mesh.vertices.len())?;
        self.length(mesh.indices.len())?;
        for vertex in &mesh.vertices {
            for value in [
                vertex.pos.x * scale,
                vertex.pos.y * scale,
                vertex.uv.x,
                vertex.uv.y,
            ] {
                self.f32(value);
            }
            self.0.extend_from_slice(&vertex.color.to_array());
        }
        for index in &mesh.indices {
            self.0.extend_from_slice(&index.to_le_bytes());
        }
        Some(())
    }
}

fn managed(texture: TextureId) -> Option<u64> {
    match texture {
        TextureId::Managed(id) => Some(id),
        TextureId::User(_) => None,
    }
}
