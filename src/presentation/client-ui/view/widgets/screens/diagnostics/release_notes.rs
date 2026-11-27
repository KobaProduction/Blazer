use super::{CONTENT_GAP, Key, UpdateRelease, ViewFrame, modal_content_width};
use crate::view::widgets::controls::primary_action;

const MAX_NOTES_BYTES: usize = 16 * 1024;
const MAX_NOTES_LINES: usize = 256;

impl ViewFrame<'_> {
    pub(super) fn release_notes_dialog(
        &self,
        ui: &egui::Ui,
        id: egui::Id,
        release: &UpdateRelease,
    ) -> bool {
        let inset = self.layout.content_inset();
        egui::Modal::new(id)
            .frame(egui::Frame::popup(ui.style()).inner_margin(inset))
            .show(ui.ctx(), |ui| {
                let reserved = 2.0 * f32::from(inset) + 24.0;
                ui.set_width(modal_content_width(ui, 560.0, reserved));
                ui.spacing_mut().item_spacing.y = CONTENT_GAP;
                let title =
                    self.message(Key::AboutUpdateNotesTitle, &[("version", &release.version)]);
                ui.add(egui::Label::new(egui::RichText::new(title).heading()).wrap());
                self.release_notes_content(ui, &release.notes);
                ui.separator();
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true),
                    |ui| {
                        if ui.add(primary_action(ui, self.tr(Key::Close))).clicked() {
                            ui.close();
                        }
                        ui.hyperlink_to(self.tr(Key::AboutUpdateOpenRelease), &release.release_url);
                    },
                );
            })
            .should_close()
    }

    fn release_notes_content(&self, ui: &mut egui::Ui, notes: &str) {
        let end = notes.floor_char_boundary(MAX_NOTES_BYTES);
        let text = notes[..end].trim();
        if text.is_empty() {
            ui.add(egui::Label::new(self.tr(Key::AboutUpdateNotesEmpty)).wrap());
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("release-notes-content")
            .max_height((ui.ctx().content_rect().height() * 0.5).min(360.0))
            .min_scrolled_height(0.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                let mut lines = text.lines();
                for line in lines.by_ref().take(MAX_NOTES_LINES) {
                    notes_line(ui, line.trim());
                }
                if end < notes.len() || lines.next().is_some() {
                    ui.add_space(5.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(self.tr(Key::AboutUpdateNotesTruncated)).weak(),
                        )
                        .wrap(),
                    );
                }
            });
    }
}

fn notes_line(ui: &mut egui::Ui, line: &str) {
    if line.is_empty() {
        ui.add_space(5.0);
        return;
    }
    let (prefix, content) = line.split_once(' ').unwrap_or(("", line));
    match prefix {
        "-" | "*" | "+" => notes_list_item(ui, "•", content),
        "#" | "##" | "###" | "####" | "#####" | "######" => {
            ui.add(egui::Label::new(egui::RichText::new(content).strong()).wrap());
        }
        marker if ordered_list_marker(marker) => notes_list_item(ui, marker, content),
        _ => {
            ui.add(egui::Label::new(line).wrap());
        }
    }
}

fn ordered_list_marker(marker: &str) -> bool {
    marker.strip_suffix('.').is_some_and(|number| {
        !number.is_empty() && number.len() <= 9 && number.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn notes_list_item(ui: &mut egui::Ui, marker: &str, text: &str) {
    ui.horizontal_top(|ui| {
        ui.label(marker);
        ui.add(egui::Label::new(text).wrap());
    });
}
