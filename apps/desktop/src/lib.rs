//! Native desktop delivery. Mutable backend state is shared with headless delivery.

use eframe::egui;
use vpn_backend::{Backend, BackendSnapshot, ConnectionState, EngineError};

pub fn run() -> eframe::Result {
    let backend = Backend::new();
    let app = VpnApp {
        snapshot: backend.snapshot(),
        backend,
        screen: Screen::Connection,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([820.0, 520.0])
            .with_min_inner_size([640.0, 420.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native("VPN-клиент", options, Box::new(|_| Ok(Box::new(app))))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Connection,
    Subscriptions,
    Configurations,
    Routing,
    VpnSettings,
    Logs,
    Telemetry,
}

impl Screen {
    const ALL: [Self; 7] = [
        Self::Connection,
        Self::Subscriptions,
        Self::Configurations,
        Self::Routing,
        Self::VpnSettings,
        Self::Logs,
        Self::Telemetry,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Connection => "Подключение",
            Self::Subscriptions => "Подписки",
            Self::Configurations => "Конфигурации",
            Self::Routing => "Маршрутизация",
            Self::VpnSettings => "VPN",
            Self::Logs => "Журнал",
            Self::Telemetry => "Телеметрия",
        }
    }
}

struct VpnApp {
    backend: Backend,
    snapshot: BackendSnapshot,
    screen: Screen,
}

impl eframe::App for VpnApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.navigation(ui);
        ui.separator();
        ui.add_space(24.0);
        match self.screen {
            Screen::Connection => self.connection(ui),
            Screen::Subscriptions => {
                placeholder(ui, "Подписки", "Импорт подписок пока недоступен.")
            }
            Screen::Configurations => {
                placeholder(ui, "Конфигурации", "Конфигурации пока не загружены.")
            }
            Screen::Routing => placeholder(ui, "Маршрутизация", "Маршруты пока не настроены."),
            Screen::VpnSettings => placeholder(ui, "Настройки VPN", "VPN-модуль ещё не подключён."),
            Screen::Logs => self.logs(ui),
            Screen::Telemetry => self.telemetry(ui),
        }
    }
}

impl VpnApp {
    fn navigation(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for screen in Screen::ALL {
                if ui
                    .selectable_label(self.screen == screen, screen.label())
                    .clicked()
                {
                    self.screen = screen;
                }
            }
        });
    }

    fn connection(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.heading("VPN-клиент");
            ui.label(format!("Версия {}", env!("CARGO_PKG_VERSION")));
            ui.add_space(24.0);
            ui.label(status_label(self.snapshot.connection));
            ui.add_space(24.0);
            ui.add_enabled(
                self.snapshot.connect_enabled,
                egui::Button::new(egui::RichText::new("Подключиться").size(24.0))
                    .min_size(egui::vec2(300.0, 84.0)),
            );
            ui.add_space(20.0);
            ui.label("VPN-подключение пока не реализовано.");
            if ui.button("Обновить состояние").clicked() {
                self.backend.refresh_status();
                self.snapshot = self.backend.snapshot();
            }
        });
    }

    fn logs(&self, ui: &mut egui::Ui) {
        ui.heading("Локальная диагностика");
        ui.label(format!(
            "Событий в памяти: {}, вытеснено: {}",
            self.snapshot.diagnostics.buffered, self.snapshot.diagnostics.dropped
        ));
        ui.label("Сетевые адреса, подписки и секреты в журнал не записываются.");
    }

    fn telemetry(&self, ui: &mut egui::Ui) {
        ui.heading("Телеметрия");
        ui.label("Внешний экспорт отключён и пока не настроен.");
        ui.label("Диагностика хранится только в ограниченном буфере в памяти.");
        ui.add_enabled(false, egui::Button::new("Включить экспорт"));
    }
}

fn placeholder(ui: &mut egui::Ui, title: &str, message: &str) {
    ui.heading(title);
    ui.label(message);
}

fn status_label(state: Result<ConnectionState, EngineError>) -> &'static str {
    match state {
        Ok(ConnectionState::Disconnected) => "Отключён",
        Ok(ConnectionState::Connected) => "Подключён",
        Err(EngineError::EngineUnavailable) => "VPN-модуль не настроен",
        Err(EngineError::UnsupportedFeature) => "Возможность не поддерживается",
    }
}
