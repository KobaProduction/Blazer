mod connection;
mod diagnostics;
mod profiles;
mod provider_settings;
mod settings;
mod subscriptions;

pub(super) use settings::toggle;

use super::ViewFrame;
use crate::view::view_model::{Screen, SettingsPage};

impl ViewFrame<'_> {
    pub(super) fn screen(&mut self, ui: &mut egui::Ui, screen: Screen) {
        match screen {
            Screen::Home => self.connection(ui),
            Screen::Statistics => self.statistics(ui),
            Screen::Settings(SettingsPage::Logs) => self.logs(ui),
            Screen::Settings(SettingsPage::About) => self.about(ui),
            Screen::Settings(page) => self.settings(ui, page),
        }
    }
}
