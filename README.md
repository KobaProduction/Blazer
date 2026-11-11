# VPN-клиент

Заготовка desktop GUI-приложения на Rust. Постоянное название пока не выбрано.
Репозиторий: [KobaProduction/vpn-client](https://github.com/KobaProduction/vpn-client).

Текущий целевой вариант — Windows x64 (MSVC). Окно показывает отсутствие
настроенного VPN-модуля; подключение недоступно. Приложение не создаёт туннель,
не меняет маршруты/DNS и не требует прав администратора на этом этапе.

## Установка и запуск

Нужны Git, PowerShell 7 (`pwsh`), rustup и Visual Studio Build Tools с компонентами
Desktop development with C++, MSVC x64 и Windows SDK. Версия Rust закреплена
в `rust-toolchain.toml`; rustup загрузит её и необходимые компоненты.

```powershell
cd C:\Users\Koba\RustroverProjects\vpn-client
git config --local core.hooksPath .githooks
cargo run --locked -p vpn-client
```

Откройте корневую папку в RustRover как Cargo workspace.

## Сборка и проверка

```powershell
cd C:\Users\Koba\RustroverProjects\vpn-client
pwsh -NoProfile -File scripts/verify.ps1
```

Команда проверяет архитектурные зависимости, формат, Clippy, компиляцию,
тесты и release-сборку для `x86_64-pc-windows-msvc`.
Результат: `target/x86_64-pc-windows-msvc/release/vpn-client.exe`.
Тот же gate обязателен в pre-push hook и GitHub Actions.
Отдельные проверки и правила локального цикла: [разработка](docs/development.md).

## Карта проекта

| Область | Путь |
| --- | --- |
| Предметная область без GUI/OS-зависимостей | `crates/domain` |
| Сценарии и порты приложения | `crates/application` |
| Реализации внешних портов | `crates/infrastructure` |
| GUI (egui/eframe), представление и composition root | `apps/desktop` |
| Правила для AI-агентов | [AGENTS.md](AGENTS.md) |
| Направление зависимостей и границы | [архитектура](docs/architecture.md) |
| Поддерживаемый target и будущие платформы | [платформы](docs/platforms.md) |

На этапе заготовки DEV/TEST/production не имеют внешних сервисов или секретов.
`.env.example` описывает отсутствие runtime-конфигурации; `.env` не загружается.
Задачи и дальнейшие решения ведутся в GitHub Issues.
