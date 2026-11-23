# Blazer

Нативный VPN-клиент на Rust с общим адаптивным интерфейсом egui.
Состояние сборок Windows, Android, Linux, macOS и iOS описано в
[платформенном контракте](docs/platforms.md).
Репозиторий: [KobaProduction/Blazer](https://github.com/KobaProduction/Blazer).
Собственный код — [MIT](LICENSE).

Цель — клиент с собственным ядром BlazeCore, низким расходом ресурсов,
подписками и знакомым управлением. Полный замысел и критерии:
[product requirements](docs/product-requirements.md);
поведение референса и границы паритета: [Happ](docs/reference-behavior.md).

В Windows проверены импорт и обновление подписок, сохранение каталога
через DPAPI и локальный SOCKS5/HTTP-прокси через BlazeCore. GUI и консольный
интерфейс используют одно ядро клиента. Режим TUN в Windows подключается
к установленной службе BlazeCoreTun. Режим «Системный прокси» подключает
локальный HTTP-прокси BlazeCore к пользовательским настройкам Windows.
Собственные правила маршрутизации и полные Xray-конфиги с `AsIs` и балансировщиками
`roundRobin` и `leastPing` проверены реальным трафиком Windows. Для `leastPing`
ядро клиента выполняет общие проверки observatory и переключает новые соединения
по состоянию выходов, сохраняя открытые потоки. Конфигурация сохраняется одним
групповым профилем с исходным JSON; подписки, Base64-обновление и ручной импорт
используют общий сценарий. Поддержанные поля и границы проверки описаны в
[приёмке маршрутизации и балансировщиков](docs/xray-balancer-acceptance.md).

Android использует тот же Rust-наблюдатель через JNI selector API одной сессии
BlazeCore TUN. На Android 15 x86_64 проверены переключение leastPing при отказе,
резервный выход, восстановление и сохранение открытого потока. В доверенном
наборе закреплены библиотеки `x86_64` и `arm64-v8a`; сетевой прогон физического
ARM64-устройства и Apple Packet Tunnel остаётся отдельной приёмкой.

## Установка и запуск

Текущий проверяемый target — Windows x64 MSVC. Нужны Git, PowerShell 7,
rustup, Visual Studio Build Tools с Desktop development with C++, MSVC x64
и Windows SDK. До первого Cargo на свежем checkout установить pinned toolchain:

```powershell
cd C:\Users\Koba\VPN\Blazer
rustup toolchain install 1.95.0-x86_64-pc-windows-msvc --profile minimal --component rustfmt,clippy
git config --local core.hooksPath .githooks
cargo run --locked -p blazer --bin blazer
```

Для подключения в Windows поместите закреплённый `blazecore.exe` рядом с
`blazer.exe`. Перед запуском ядра клиент сверяет его SHA-256.

Откройте корень в RustRover как Cargo workspace. GUI использует egui/eframe + Glow.
Для консольного запуска того же приложения передайте `--cli`:

```powershell
cd C:\Users\Koba\VPN\Blazer
cargo run --locked -p blazer -- --cli status
```

`status` сообщает состояние и доступность режимов, затем завершает процесс.
`profiles`, `subscriptions`, `settings` и `diagnostics` читают состояние
ядра клиента. `profile select|test|remove <uuid>` работает с каталогом,
`subscription rename <uuid> <название>` меняет имя без скачивания подписки.
`profiles test` и `subscription test <uuid>` проверяют серверы всего каталога
или выбранной подписки. Для UDP и QUIC Windows выполняет контрольный запрос
через локальный прокси отдельного экземпляра BlazeCore.
При активном Windows TUN проверка текущего профиля использует действующий
туннель; второе подключение WireGuard или AmneziaWG не создаётся.
В «Настройки → Пинг» сохраняются метод (Авто, TCP, ICMP, через прокси),
GET/HEAD, HTTP(S)-адрес проверки, таймаут 250–10000 мс, прогрев,
1–5 измерений и результат: последнее значение, минимум, среднее или медиана.
ICMP использует API Windows; для него активный TUN нужно отключить.
По умолчанию используются HEAD по HTTPS и два измерения после отдельного прогрева.
Для HTTPS прогрев обязателен, для HTTP его можно отключить. Время TCP/TLS
при установке или переустановлении соединения исключается из прогретых замеров.
Если ресурс не позволяет переиспользовать соединение, проверка сообщает об этом.
Чистый TCP-метод измеряет установление соединения с сервером; для измерения
запросов по уже открытому соединению используйте HTTP/HTTPS через прокси.
Те же настройки читает `blazer --cli ping-settings`. Для изменения передайте
метод, GET/HEAD, таймаут, `warmup`/`cold`, число измерений и стратегию;
адрес проверки читается из stdin, например:

```powershell
'https://connectivitycheck.gstatic.com/generate_204' | .\dist\windows\client\blazer.exe --cli ping-settings proxy head 5000 warmup 3 median
```

Полный список команд показывает `blazer --cli --help`. URL и конфиги
передаются через стандартный ввод до EOF.
В Windows файлы WireGuard и AmneziaWG `.conf` можно открыть через
«Добавить конфигурацию → Открыть файл». Ссылки Amnezia `vpn://` с готовым
профилем WireGuard или AmneziaWG можно вставить, загрузить из файла
или считать из QR-кода с этой ссылкой. Там же принимаются нативные
клиентские профили BlazeCore в JSON и YAML. Те же форматы принимает
`blazer --cli import`.
Для локального прокси через проверенный BlazeCore есть команда `proxy`,
для системного прокси Windows — `system-proxy`. Обе команды работают до Enter или EOF.
Системный режим временно отключает PAC/WPAD и восстанавливает прежние
настройки при отключении и завершении клиента. Приложения должны поддерживать
системный прокси; для всего IP-трафика используйте TUN.
Профиль маршрутизации для него можно сохранить и назначить из headless.

```powershell
@'
dns automatic
block domain blocked.example
direct cidr 192.0.2.0/24
'@ | cargo run --locked -p blazer -- --cli routing add "Локальная сеть"
cargo run --locked -p blazer -- --cli routing
$profileUuid = Read-Host 'UUID профиля из строки routing_profile'
cargo run --locked -p blazer -- --cli routing assign global $profileUuid enabled
```

В последней команде укажите UUID из `routing`. Настройки применяются при
следующем подключении LocalProxy.

Для системного туннеля есть команда `tun` и [инструкция по установке BlazeCoreTun](docs/platforms.md).
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

Результат находится в `target/android/blazer-debug.apk`.
Идентификатор приложения — `com.kobaproduction.blazer`. По умолчанию в APK
входят `arm64-v8a` и `x86_64`. Можно выбрать один ABI через `-Abi`.
Скрипт использует локальную debug-подпись. Для установки на тестовый
эмулятор или подключённое устройство используйте `adb install -r`.
Обычный запуск Blazer открывает общий адаптивный интерфейс.

Для сборки с системным VPN передайте `-BlazeCoreLibraries <папка>`. Папка содержит
`<ABI>/libblazecore.so` из доверенной локальной сборки форка и `manifest.json`
с полями `revision`, `source_tree`, `version`, `capabilities_bits` и `artifacts`.
Каждый элемент `artifacts` указывает ABI, target, файл и SHA-256. Скрипт сверяет
исходный tree, версию, selector-контракт и закреплённый дайджест каждого ABI перед упаковкой. При запуске
Android дополнительно проверяет версию загруженной библиотеки.
Текущий статус функций описан в [платформенном контракте](docs/platforms.md).

## Пакеты Windows

Соберите пакет Blazer для Windows x64:

```powershell
cd C:\Users\Koba\VPN\Blazer
pwsh -NoProfile -File scripts/build-windows.ps1
```

Один `dist/windows/client/blazer.exe` запускает окно без параметров и консоль
через `--cli`. Debug-сборка также поддерживает `--demo` и
[захват PNG](docs/frontend.md#проверки-и-доказанность).

По умолчанию скрипт собирает Debug. Для Release передайте
`-Configuration Release`.
`-Configuration Release -Installer` дополнительно собирает единый Windows
Setup с клиентом, закреплённым BlazeCore, Wintun и управлением службой туннеля.
Этот preview требует чистого checkout. Для незакоммиченных исходников есть
`-Configuration Release -DevelopmentInstaller`: неустанавливаемый compile-check
артефакт с текущим клиентом, без публикации и без заявления immutable identity.
Установщик использует мастер NSIS с русским и английским интерфейсом;
целевой каталог — `C:\Program Files\Blazer`.
`-OutputDirectory` меняет общую родительскую папку пакетов.
Пакет содержит `LICENSE`. Скрипт ищет `blazecore.exe` с
закреплённым SHA-256 в Windows release-пакете или debug-сборке соседнего BlazeCore
и в `target/x86_64-pc-windows-msvc/release`. Параметр `-BlazeCorePath` задаёт
явный путь. Скрипт кладёт ядро рядом с клиентом. Лицензии и уведомления
берутся из поставки ядра. Путь к лицензии можно указать через `-BlazeCoreLicensePath`.

## Версии и обновления

Версию стабильного выпуска задаёт Git-тег `vX.Y`, например `v0.1`. В Windows её показывают `blazer --version`,
свойства EXE и экран «О программе». Для обычных локальных сборок версия берётся
из `Cargo.toml`. На Android `-TaggedVersion` берёт версию из тега для отладочного APK,
`-Distribution Direct` собирает подписанный APK для GitHub. Канал Google Play
открывает страницу приложения в магазине. iOS использует версию тега и отдельный
номер сборки, а обновления поставляются через App Store или TestFlight.

`-Configuration Release -ReleaseArchive` собирает Windows Setup из чистого checkout
тега для [GitHub Releases](https://github.com/KobaProduction/Blazer/releases).
Пуш в `main` обновляет единственный [Windows prerelease](https://github.com/KobaProduction/Blazer/releases/tag/preview).
Стабильный Setup публикуется по тегу `vX.Y`. Номер preview — количество
first-parent коммитов Git; полный коммит отличает сборки с одинаковым `X.Y`.
В Windows проверьте обновления в разделе «О программе», скачайте проверенный
установщик и подтвердите установку в мастере. Отмена мастера или UAC оставляет
приложение открытым. [Установка и границы проверки](docs/windows-installation.md) ·
[Контракт выпусков](docs/architecture.md#версионирование-и-выпуски)

## Локальные проверки Windows

```powershell
cd C:\Users\Koba\VPN\Blazer
pwsh -NoProfile -File scripts/verify.ps1
```

Gate проверяет границы слоёв и отсутствие GUI в headless-графе,
формат, Clippy (сложность максимум 15), компиляцию, тесты и release-build.
Результаты Cargo находятся в `target/x86_64-pc-windows-msvc/release`.
Локальный pre-push обязателен.
GitHub Actions собирает и публикует релизы. Проверки разработки выполняются локально.
Локальный быстрый цикл и отдельные этапы: [development](docs/development.md).

## Карта

| Область | Путь |
| --- | --- |
| Домен | `src/domain` |
| Сценарии и типизированные порты | `src/application` |
| Внешние адаптеры | `src/infrastructure` |
| Ядро клиента, состояние и диагностика | `src/client-core` |
| Общие контроллеры и модели представления | `src/presentation/shared` |
| Общий нативный интерфейс и компоновки | `src/presentation/client-ui` |
| Платформенные оболочки | `src/platform/{windows,android,linux,macos,ios}` |
| Консольный интерфейс | `src/presentation/client-cli` |

У проекта один общий каталог `src/`. В каждом пакете `Cargo.toml` явно
указывает его входной файл. Windows выбирает режим в одном `main.rs`,
Android-библиотека собирается из `src/platform/android/android.rs`.
| AI-agent entry map | [AGENTS.md](AGENTS.md) |
| Архитектура и integration boundaries | [architecture](docs/architecture.md) |
| Windows и будущие платформы | [platforms](docs/platforms.md) |
| Измерение ресурсов | [performance](docs/performance.md) |

OTel wiring включена, network export выключен и не настраивается через
OTEL env. Локальный ring ограничен 128 enum events без подписок/URL/IP/секретов.
`.env` не загружается; DEV/TEST/production пока не имеют внешнего backend.
Проверки Android не заменяют отдельную приёмку Linux, macOS, iOS или OpenWrt.
Задачи и незавершённая работа ведутся в GitHub Issues, не в Markdown backlog.
