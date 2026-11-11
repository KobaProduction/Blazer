//! Desktop delivery layer and composition root.

use eframe::egui;
use vpn_application::{GetConnectionStatus, TunnelUnavailable};
use vpn_domain::ConnectionState;
use vpn_infrastructure::UnavailableTunnel;

/// Compose the application and start a native desktop window.
pub fn run() -> eframe::Result {
    let query = GetConnectionStatus::new(UnavailableTunnel);
    let app = VpnApp {
        state: query.execute(),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([520.0, 340.0])
            .with_min_inner_size([420.0, 280.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native("VPN-клиент", options, Box::new(|_| Ok(Box::new(app))))
}

struct VpnApp {
    state: Result<ConnectionState, TunnelUnavailable>,
}

impl eframe::App for VpnApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.add_space(32.0);
        ui.vertical_centered(|ui| {
            ui.heading("VPN-клиент");
            ui.label(format!("Версия {}", env!("CARGO_PKG_VERSION")));
            ui.add_space(24.0);
            ui.label(status_label(self.state));
            ui.add_space(16.0);
            ui.add_enabled(false, egui::Button::new("Подключиться"));
            ui.add_space(16.0);
            ui.label("VPN-подключение пока не реализовано.");
        });
    }
}

fn status_label(state: Result<ConnectionState, TunnelUnavailable>) -> &'static str {
    match state {
        Ok(ConnectionState::Disconnected) => "Отключён",
        Ok(ConnectionState::Connected) => "Подключён",
        Err(TunnelUnavailable) => "VPN-модуль не настроен",
    }
}
