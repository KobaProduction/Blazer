use vpn_presentation::session::{ConnectionMode, ConnectionStatus};

use super::ViewFrame;
use super::controls::{selector, selector_option};
use vpn_presentation::localization::Key;
use vpn_presentation::model::{self as viewmodel, Intent};

pub(super) const CONNECTION_CONTROL_WIDTH: f32 = 296.0;

impl ViewFrame<'_> {
    pub(super) fn mode_picker(&mut self, ui: &mut egui::Ui) {
        let selected = self.tr(picker_mode_key(self.snapshot.selected_mode));
        selector(
            ui,
            "connection-mode",
            CONNECTION_CONTROL_WIDTH,
            selected,
            |ui| {
                self.mode_option(ui, ConnectionMode::LocalProxy);
                if self.snapshot.mode_capabilities.system_proxy
                    || self.snapshot.selected_mode == ConnectionMode::SystemProxy
                {
                    self.mode_option(ui, ConnectionMode::SystemProxy);
                }
                ui.separator();
                ui.weak(self.tr(Key::ModeTun));
                ui.indent("tun-modes", |ui| self.mode_option(ui, ConnectionMode::Tun));
            },
        );
    }

    fn mode_option(&mut self, ui: &mut egui::Ui, mode: ConnectionMode) {
        let selected = self.snapshot.selected_mode == mode;
        let available = viewmodel::mode_available(mode, self.snapshot.mode_capabilities);
        let blocked = self.snapshot.busy
            || self.saving_mode
            || self.snapshot.cleanup_required
            || self.snapshot.active_mode.is_some();
        let disabled_reason = if !available {
            Key::ModeUnavailable
        } else if self.snapshot.busy || self.saving_mode {
            Key::Working
        } else {
            Key::ModeFinishFirst
        };
        let button = selector_option(ui, selected, self.tr(picker_mode_key(mode)));
        if ui
            .add_enabled(available && (selected || !blocked), button)
            .on_disabled_hover_text(self.tr(disabled_reason))
            .clicked()
        {
            if !selected {
                self.emit(Intent::ChangeMode(mode));
            }
            ui.close();
        }
    }

    pub(super) fn connection_details(&self, ui: &mut egui::Ui) {
        if let Some(active) = self.snapshot.active_mode
            && active != self.snapshot.selected_mode
        {
            let mode = self.tr(viewmodel::mode_key(active));
            ui.weak(self.message(Key::ActiveMode, &[("mode", mode)]));
        }
        if self.snapshot.cleanup_required {
            ui.label(self.tr(Key::CleanupRequired));
        } else if self.snapshot.connection == ConnectionStatus::Connected {
            self.proxy_endpoints(ui);
        }
    }

    fn proxy_endpoints(&self, ui: &mut egui::Ui) {
        let socks = self.snapshot.local_proxy.as_deref();
        let http = self.snapshot.http_proxy.as_deref();
        if let (Some(socks), Some(http)) = (socks, http)
            && socks == http
        {
            ui.label(self.message(Key::CombinedEndpoint, &[("endpoint", socks)]));
            return;
        }
        for (key, endpoint) in [(Key::SocksEndpoint, socks), (Key::HttpEndpoint, http)] {
            if let Some(endpoint) = endpoint {
                ui.label(self.message(key, &[("endpoint", endpoint)]));
            }
        }
    }
}

fn picker_mode_key(mode: ConnectionMode) -> Key {
    match mode {
        ConnectionMode::LocalProxy => Key::CatalogProxyMode,
        ConnectionMode::Tun => Key::CatalogBlazeCoreTun,
        ConnectionMode::SystemProxy => Key::ModeSystemProxy,
    }
}
