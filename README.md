# VPN-клиент

Нативное Rust-приложение под временным названием «VPN-клиент».
Репозиторий: [KobaProduction/vpn-client](https://github.com/KobaProduction/vpn-client).
Собственный код — [MIT](LICENSE); приватность репозитория сейчас не меняется.

Цель — клиент с собственным модифицированным Shoes, низким расходом ресурсов,
подписками и знакомым управлением. Полный замысел и критерии:
[product requirements](docs/product-requirements.md);
поведение референса и границы паритета: [Happ](docs/reference-behavior.md).

Сейчас подготовлена основа: native GUI с большой недоступной кнопкой
подключения, общий backend, headless `status`, typed import/engine ports,
1 MiB input guard и локальная bounded диагностика. Engine/normalizer
явно недоступны: VPN, downloader, parser, persistence и маршрутизация
не реализованы. Системные routes/DNS не меняются.

## Установка и запуск

Текущий проверяемый target — Windows x64 MSVC. Нужны Git, PowerShell 7,
rustup, Visual Studio Build Tools с Desktop development with C++, MSVC x64
и Windows SDK. До первого Cargo на свежем checkout установить pinned toolchain:

```powershell
cd C:\Users\Koba\RustroverProjects\vpn-client
rustup toolchain install 1.95.0-x86_64-pc-windows-msvc --profile minimal --component rustfmt,clippy
git config --local core.hooksPath .githooks
cargo run --locked -p vpn-client
```

Откройте корень в RustRover как Cargo workspace. GUI — egui/eframe + Glow;
JS, Electron и WebView не используются. Headless без GUI dependencies:

```powershell
cd C:\Users\Koba\RustroverProjects\vpn-client
cargo run --locked -p vpn-headless -- status
```

Headless сообщает `engine=unavailable`, `connect_enabled=false`,
`telemetry_export=not_configured`. Он завершается после запроса;
это не установленный сервис и не работающий VPN.

## Локальные проверки и сборка

```powershell
cd C:\Users\Koba\RustroverProjects\vpn-client
pwsh -NoProfile -File scripts/verify.ps1
```

Gate проверяет границы шести пакетов и отсутствие GUI в headless-графе,
формат, Clippy (сложность максимум 15), компиляцию, тесты и release-build:
`target/x86_64-pc-windows-msvc/release/vpn-client.exe` и `vpn-headless.exe`
в той же папке. Локальный pre-push обязателен.
По решению владельца CI не используется и его ожидание не входит в разработку.
Локальный быстрый цикл и отдельные этапы: [development](docs/development.md).

## Карта

| Область | Путь |
| --- | --- |
| Домен | `crates/domain` |
| Сценарии и типизированные порты | `crates/application` |
| Внешние адаптеры | `crates/infrastructure` |
| Общий backend, состояние, OTel/diagnostics | `crates/backend` |
| Native GUI и headless delivery | `apps/desktop`, `apps/headless` |
| AI-agent entry map | [AGENTS.md](AGENTS.md) |
| Архитектура и integration boundaries | [architecture](docs/architecture.md) |
| Windows и будущие платформы | [platforms](docs/platforms.md) |
| Измерение ресурсов | [performance](docs/performance.md) |

OTel wiring включена, network export выключен и не настраивается через
OTEL env. Локальный ring ограничен 128 enum events без подписок/URL/IP/секретов.
`.env` не загружается; DEV/TEST/production пока не имеют внешнего backend.
Целевые Linux/macOS/Android/iOS/OpenWrt требуют отдельной build/runtime приёмки.
Задачи и незавершённая работа ведутся в GitHub Issues, не в Markdown backlog.
