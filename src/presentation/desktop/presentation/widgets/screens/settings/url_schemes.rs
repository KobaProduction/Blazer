use eframe::egui;
use vpn_presentation::import::IMPORT_LINK_TEMPLATE;
use vpn_presentation::localization::Key;

use super::{ViewFrame, group};

impl ViewFrame<'_> {
    pub(super) fn url_schemes(&self, ui: &mut egui::Ui) {
        group(ui, Some(self.tr(Key::SchemeImport)), |ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(IMPORT_LINK_TEMPLATE).monospace())
                    .selectable(true),
            );
            ui.label(self.tr(Key::SchemeImportHelp));
        });
    }
}
