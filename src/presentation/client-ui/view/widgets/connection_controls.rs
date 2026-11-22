use vpn_presentation::session::{ConnectionMode, ConnectionStatus, ModeCapabilities};

use super::ViewFrame;
use super::controls::{selector, selector_option};
use vpn_presentation::localization::Key;
use vpn_presentation::model::{self as viewmodel, Intent};

pub(super) const CONNECTION_CONTROL_WIDTH: f32 = 296.0;

impl ViewFrame<'_> {
    pub(super) fn mode_picker(&mut self, ui: &mut egui::Ui) {
        let selected = self.tr(picker_mode_key(self.snapshot.selected_mode));
        let modes = picker_modes(self.snapshot.mode_capabilities);
        if modes.is_empty() || (modes.len() == 1 && modes.contains(&self.snapshot.selected_mode)) {
            ui.label(selected);
            return;
        }
        selector(
            ui,
            "connection-mode",
            CONNECTION_CONTROL_WIDTH,
            selected,
            |ui| {
                for mode in modes {
                    self.mode_option(ui, mode);
                }
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

fn picker_modes(capabilities: ModeCapabilities) -> Vec<ConnectionMode> {
    let proxy = if capabilities.system_proxy {
        Some(ConnectionMode::SystemProxy)
    } else if capabilities.supports(ConnectionMode::LocalProxy) {
        Some(ConnectionMode::LocalProxy)
    } else {
        None
    };
    proxy
        .into_iter()
        .chain(capabilities.tun.then_some(ConnectionMode::Tun))
        .collect()
}

fn picker_mode_key(mode: ConnectionMode) -> Key {
    match mode {
        ConnectionMode::LocalProxy => Key::ModeProxy,
        ConnectionMode::SystemProxy => Key::ModeSystemProxy,
        ConnectionMode::Tun => Key::CatalogBlazeCoreTun,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_proxy_is_a_distinct_action_with_a_truthful_label() {
        let capabilities = ModeCapabilities {
            local_proxy: true,
            local_http_proxy: true,
            system_proxy: true,
            tun: true,
        };
        assert_eq!(
            picker_modes(capabilities),
            [ConnectionMode::SystemProxy, ConnectionMode::Tun]
        );
        assert_eq!(picker_mode_key(ConnectionMode::LocalProxy), Key::ModeProxy);
        assert_eq!(
            picker_mode_key(ConnectionMode::SystemProxy),
            Key::ModeSystemProxy
        );
        assert_eq!(
            picker_mode_key(ConnectionMode::Tun),
            Key::CatalogBlazeCoreTun
        );
    }

    #[test]
    fn local_proxy_remains_available_on_platforms_without_system_proxy() {
        assert_eq!(
            picker_modes(ModeCapabilities {
                local_http_proxy: true,
                tun: true,
                ..ModeCapabilities::default()
            }),
            [ConnectionMode::LocalProxy, ConnectionMode::Tun]
        );
        assert_eq!(
            picker_modes(ModeCapabilities {
                system_proxy: true,
                ..ModeCapabilities::default()
            }),
            [ConnectionMode::SystemProxy]
        );
        assert!(picker_modes(ModeCapabilities::default()).is_empty());
    }
}
