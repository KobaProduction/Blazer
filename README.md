# Blazer

Нативный VPN-клиент на Rust для Windows и Android.
Репозиторий: [KobaProduction/Blazer](https://github.com/KobaProduction/Blazer).
Собственный код — [MIT](LICENSE); приватность репозитория сейчас не меняется.

Цель — клиент с собственным ядром Blaze Core на базе Shoes, низким расходом ресурсов,
подписками и знакомым управлением. Полный замысел и критерии:
[product requirements](docs/product-requirements.md);
поведение референса и границы паритета: [Happ](docs/reference-behavior.md).

В Windows проверены импорт и обновление подписок, сохранение каталога
через DPAPI и локальный SOCKS5/HTTP-прокси через Shoes. Через единый backend
работают GUI и headless. Windows TUN с Wintun доступен для локальной приёмки
по явной конфигурации. Системный прокси остаётся недоступным.

## Установка и запуск

Текущий проверяемый target — Windows x64 MSVC. Нужны Git, PowerShell 7,
rustup, Visual Studio Build Tools с Desktop development with C++, MSVC x64
и Windows SDK. До первого Cargo на свежем checkout установить pinned toolchain:

```powershell
cd C:\Users\Koba\VPN\Blazer
rustup toolchain install 1.95.0-x86_64-pc-windows-msvc --profile minimal --component rustfmt,clippy
git config --local core.hooksPath .githooks
cargo run --locked -p vpn-client --bin vpn-client
```

Для подключения в Windows поместите закреплённый `shoes.exe` рядом с
`vpn-client.exe`. Перед запуском ядра клиент сверяет его SHA-256.

Откройте корень в RustRover как Cargo workspace. GUI — egui/eframe + Glow;
JS, Electron и WebView не используются. Headless без GUI dependencies:

```powershell
cd C:\Users\Koba\VPN\Blazer
cargo run --locked -p vpn-headless -- status
```

`status` сообщает состояние и доступность режимов, затем завершает процесс.
`profiles`, `subscriptions`, `settings` и `diagnostics` читают состояние
общего backend. `profile select|test|remove <uuid>` работает с каталогом,
`subscription rename <uuid> <название>` меняет имя без скачивания подписки.
`profiles test` и `subscription test <uuid>` проверяют TCP-доступность
всего каталога или серверов выбранной подписки.
Полный список команд показывает `vpn-headless --help`. URL и конфиги
передаются через стандартный ввод до EOF.
Для локального прокси через проверенный Shoes есть команда `proxy`.
Профиль маршрутизации для него можно сохранить и назначить из headless.

```powershell
@'
dns automatic
block domain blocked.example
direct cidr 192.0.2.0/24
'@ | cargo run --locked -p vpn-headless -- routing add "Локальная сеть"
cargo run --locked -p vpn-headless -- routing
$profileUuid = Read-Host 'UUID профиля из строки routing_profile'
cargo run --locked -p vpn-headless -- routing assign global $profileUuid enabled
```

В последней команде укажите UUID из `routing`. Настройки применяются при
следующем подключении LocalProxy.

Для системного туннеля есть команда `tun` и [настройка Wintun](docs/platforms.md).
Доступные режимы и переменные окружения описаны в [архитектуре](docs/architecture.md).

## Сборка для Android

На Windows сначала проверьте, где уже установлены Android SDK и Java.
Используйте один каталог SDK с правом записи вне рабочих каталогов Codex,
например `%LOCALAPPDATA%\Android\Sdk`. Для сборки подходит существующая
установка Java 17 или новее с `javac` и `keytool` в каталоге `bin`.
Храните SDK, Java и данные эмулятора вне рабочих каталогов Codex.

1. В SDK Manager установите Android SDK Platform 35 (`platforms;android-35`),
   Build-Tools 35.0.1 (`build-tools;35.0.1`), NDK r27c
   (`ndk;27.2.12479018`), Platform-Tools и Command-line Tools (latest).
   Для выбора точной версии включите **Show Package Details**.
2. В переменных среды пользователя Windows задайте `ANDROID_HOME` как
   полный путь к выбранному SDK, а `JAVA_HOME` как корень действующей
   установки Java.
   В пользовательский `Path` добавьте `%ANDROID_HOME%\platform-tools` для
   `adb` и `%ANDROID_HOME%\cmdline-tools\latest\bin` для `sdkmanager`.
   Откройте новый терминал, чтобы он получил эти переменные.
3. Для тестирования на виртуальном устройстве установите Android Emulator
   и подходящий системный образ, добавьте `%ANDROID_HOME%\emulator`
   в пользовательский `Path` и оставьте AVD в `%USERPROFILE%\.android\avd`.
   Для сборки APK достаточно пакетов из шага 1.

[Переменные среды Android SDK](https://developer.android.com/tools/variables) ·
[Пакеты SDK и sdkmanager](https://developer.android.com/tools/sdkmanager)

Установите Android targets для закреплённого Rust toolchain и соберите APK.

```powershell
cd C:\Users\Koba\VPN\Blazer
rustup target add aarch64-linux-android x86_64-linux-android
pwsh -NoProfile -File scripts/build-android.ps1
```

Результат находится в `target/android/vpn-client-debug.apk`. По умолчанию в
APK входят `arm64-v8a` и `x86_64`. Можно выбрать один ABI через `-Abi`.
Скрипт использует локальную debug-подпись. Для установки на тестовый
эмулятор или подключённое устройство используйте `adb install -r`.

Для сборки с системным VPN передайте `-ShoesLibraries <папка>`. Папка содержит
`<ABI>/libshoes.so` из доверенной локальной сборки форка и `manifest.json`
с полями `revision`, `version` и `sha256`. `sha256` содержит SHA-256 для каждого
ABI. Скрипт сверяет ревизию, версию и дайджесты перед упаковкой. При запуске
Android дополнительно проверяет версию загруженной библиотеки.
Текущий статус функций описан в [платформенном контракте](docs/platforms.md).

## Пакеты Windows

Соберите два отдельных пакета для Windows x64:

```powershell
cd C:\Users\Koba\VPN\Blazer
pwsh -NoProfile -File scripts/build-windows.ps1 -Variant Both
```

| Запускаемый файл | Назначение |
| --- | --- |
| `dist/windows/client/vpn-client.exe` | Рабочий клиент с сохранением каталога через DPAPI, загрузкой подписок и подключением через Shoes. |
| `dist/windows/demo/vpn-client-demo.exe` | Интерактивное демо с тестовыми профилями в памяти и имитацией сетевых действий. Поддерживает [захват PNG](docs/frontend.md#проверки-и-доказанность). |

Для одного пакета выберите `-Variant Client` или `-Variant Demo`.
По умолчанию скрипт собирает Debug. Для Release передайте
`-Configuration Release`.
`-OutputDirectory` меняет общую родительскую папку пакетов.
Оба пакета содержат `LICENSE`. Для рабочего клиента скрипт берёт закреплённый
`shoes.exe` из `target/x86_64-pc-windows-msvc/release` или из `-ShoesPath`,
сверяет SHA-256 и кладёт ядро рядом с клиентом. Для `LICENSE-SHOES` скрипт
сначала ищет `../BlazeCore/LICENSE`, затем `../shoes/LICENSE` и сохранённую
копию `dist/windows/client/LICENSE-SHOES`. Параметр `-ShoesLicensePath`
задаёт явный путь. Для демо достаточно toolchain из раздела установки.

## Локальные проверки Windows

```powershell
cd C:\Users\Koba\VPN\Blazer
pwsh -NoProfile -File scripts/verify.ps1
```

Gate проверяет границы слоёв и отсутствие GUI в headless-графе,
формат, Clippy (сложность максимум 15), компиляцию, тесты и release-build.
Результаты Cargo находятся в `target/x86_64-pc-windows-msvc/release`.
Локальный pre-push обязателен.
По решению владельца CI не используется и его ожидание не входит в разработку.
Локальный быстрый цикл и отдельные этапы: [development](docs/development.md).

## Карта

| Область | Путь |
| --- | --- |
| Домен | `src/domain` |
| Сценарии и типизированные порты | `src/application` |
| Внешние адаптеры | `src/infrastructure` |
| Общий backend, состояние и диагностика | `src/backend` |
| Общие контроллеры и модели представления | `src/presentation/shared` |
| Точки запуска | `src/presentation/desktop`, `src/presentation/android`, `src/presentation/headless` |

У проекта один общий каталог `src/`. В каждом пакете `Cargo.toml` явно
указывает его входной файл. Desktop и headless запускаются из своего
`main.rs`, Android-библиотека собирается из `src/presentation/android/android.rs`.
| AI-agent entry map | [AGENTS.md](AGENTS.md) |
| Архитектура и integration boundaries | [architecture](docs/architecture.md) |
| Windows и будущие платформы | [platforms](docs/platforms.md) |
| Измерение ресурсов | [performance](docs/performance.md) |

OTel wiring включена, network export выключен и не настраивается через
OTEL env. Локальный ring ограничен 128 enum events без подписок/URL/IP/секретов.
`.env` не загружается; DEV/TEST/production пока не имеют внешнего backend.
Проверки Android не заменяют отдельную приёмку Linux, macOS, iOS или OpenWrt.
Задачи и незавершённая работа ведутся в GitHub Issues, не в Markdown backlog.
