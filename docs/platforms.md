# Платформенный контракт

| Платформа | Обязательство текущего этапа |
| --- | --- |
| Windows x64, `x86_64-pc-windows-msvc` | Локальные проверки и сборка native GUI + headless |
| Linux x64 | Общий egui/Glow, Secret Service, локальный прокси и управляемый TUN BlazeCore. Системный TUN требует отдельной сетевой приёмки |
| macOS | Metal, общий egui, Keychain и Packet Tunnel для сборки `macos-tunnel`. Сборка, активация и сетевой прогон требуют Mac с Xcode |
| Android 8.0 и новее (API 26), `arm64-v8a` и `x86_64` | APK и нативный интерфейс поверх общего клиентского контроллера. Сетевой runtime принимает отдельный платформенный тест |
| iOS | Metal, общий egui и Keychain. Сборка для устройства включает Packet Tunnel Provider. Подпись и сетевой прогон выполняются на Apple |
| OpenWrt / роутеры | Будущий демон на `client-core` с веб-интерфейсом управления; отдельно target/ABI/libc и ресурсный профиль |
| Другие устройства | Поддержка определяется отдельным целевым контрактом |

## Каналы мобильных обновлений

| Канал | Версия и установка |
| --- | --- |
| Android, APK с GitHub | `-Distribution Direct` пакует `Blazer-X.Y-android.apk` из тега, подписывает его заданным ключом и прописывает канал `direct`. Кнопка в Android-оболочке открывает GitHub Releases в браузере. Скачанный APK устанавливает Android с подтверждением пользователя. |
| Android, Google Play | Установки с `com.android.vending` и сборки с метаданными `google-play` открывают карточку приложения в Google Play. Скачивание и установка идут через Play. Для выпуска ещё нужны AAB, Play App Signing и настройка треков. |
| iOS, App Store / TestFlight | App Store Connect получает `CFBundleShortVersionString` в формате `X.Y.0` и отдельный `CFBundleVersion`. Обновление выполняет App Store или TestFlight. |

Android использует одно application ID `com.kobaproduction.blazer`. Для перехода
между каналами с сохранением данных нужны совместимая подпись и возрастающий
`versionCode`. Upload key Google Play и app signing key выполняют разные роли.
Решение о совместимой подписи принимается до первой публикации. Установки
из Google Play используют обновление магазина. [Как Android обновляет приложения](https://developer.android.com/google/play/app-updates) · [Play App Signing](https://developer.android.com/studio/publish/app-signing) · [Правила обновлений Google Play](https://support.google.com/googleplay/android-developer/answer/16559646)

Для прямого APK укажите `-SigningKeyStore`, `-SigningKeyAlias` и передайте
пароль через `BLAZER_ANDROID_STORE_PASSWORD`. Если пароль ключа отличается,
передайте его через `BLAZER_ANDROID_KEY_PASSWORD`. Отладочный APK использует
локальный debug-ключ. Параметры `-TaggedVersion` и `-VersionCode` позволяют
проверить версию на тестовом устройстве до публикации. [apksigner](https://developer.android.com/tools/apksigner) · [Версионирование Android](https://developer.android.com/studio/publish/versioning)

`build-apple.sh ios-simulator --tagged-version` и
`build-apple.sh ios-device --tagged-version` переносят версию тега в `Info.plist`.
В сборке для устройства приложение и его расширение получают одну версию и
один номер из `BLAZER_APPLE_BUILD_NUMBER`. Для повторной загрузки той же версии
нужен новый номер сборки. Архив и установка с App Store Connect проходят
отдельную приёмку.
[Версия и номер сборки Apple](https://developer.apple.com/documentation/Xcode/preparing-your-app-for-distribution)

## Системный прокси Windows

Режим `SystemProxy` доступен в GUI и через `blazer --cli system-proxy`.
Он направляет HTTP/HTTPS приложений, использующих пользовательские параметры
прокси Windows, через слушатель BlazeCore. `LocalProxy` доступен из CLI без
изменения настроек ОС. Системный прокси не заменяет TUN для произвольного IP-трафика.

Настройки прокси, исключения и PAC/WPAD сохраняются перед включением.
Скрытый процесс восстановления отслеживает канал клиента; при отключении и
разрыве канала он возвращает прежние значения. Второй экземпляр Blazer
не может одновременно захватить системный прокси той же учётной записи.
Защищённая DPAPI запись восстановления позволяет следующему запуску Blazer
очистить прокси после завершения всего дерева процессов или перезагрузки Windows.
Системная политика с общими настройками прокси для компьютера не поддерживается:
клиент сообщает ошибку, не изменяя настройки администратора.

Повторяемая проверка Windows-пакета:

```powershell
python scripts/test-windows-system-proxy.py --exe dist/windows/client/blazer.exe
```

Проверка временно меняет пользовательские настройки прокси, использует отдельный
каталог профилей и локальные сетевые фикстуры, затем восстанавливает исходные значения.

## Linux и Apple

Linux запускается через пакет `blazer-linux`. По умолчанию каталог находится
в `$XDG_DATA_HOME/blazer` или `$HOME/.local/share/blazer`. Для сохранения
профилей требуются сессия D-Bus и разблокированный Secret Service. Ключ
остаётся в Secret Service, каталог запечатывается через XChaCha20-Poly1305.
Для локального прокси разместите доверенный ELF `blazecore` рядом с клиентом
и передайте его проверенный SHA-256 и точную версию при сборке через
`BLAZER_LINUX_CORE_SHA256` и `BLAZER_LINUX_CORE_VERSION`.
На Linux x64 установите нативный toolchain Rust 1.95.0 и выполните из корня
репозитория:

```sh
rustup run 1.95.0 cargo build --locked --target x86_64-unknown-linux-gnu -p blazer-linux
```

Исполняемый файл находится в `target/x86_64-unknown-linux-gnu/debug`.
В Ubuntu x86_64 проверены открытие окна через X11, сохранение и повторное
чтение каталога с GNOME Keyring, а также передача байтов через SOCKS5,
BlazeCore и локальный HTTP CONNECT. Wayland и системный TUN ещё не приняты.

В режиме TUN закреплённый BlazeCore создаёт `blazer0` и управляет маршрутами,
правилами nftables и настройками DNS. Linux-сборке нужны `/dev/net/tun`,
системный `nft` и унаследованный `CAP_NET_ADMIN` для процесса ядра и его
дочерних команд. При отключении клиент отправляет SIGTERM и ждёт завершения,
чтобы ядро удалило свои правила. Приёмка требует ядра с поддержкой nftables
`inet` FIB и conntrack. При проверке в WSL2 6.6.87.2 создание TUN прошло,
а ядро отклонило выражение `fib` для правил захвата. [Контракт TUN BlazeCore](https://github.com/ArthurKoba/BlazeCore/blob/master/docs/src/content/docs/config/tun.md#managed-linux-desktop-capture)

Скрипт `scripts/build-apple.sh` предназначен для Mac с Xcode и
Rust 1.95.0 с соответствующим Apple target. Аргумент `macos` собирает
`Blazer.app`. Для локального прокси при сборке задайте `BLAZER_MACOS_CORE_BIN`,
`BLAZER_MACOS_CORE_SHA256` в нижнем регистре и `BLAZER_MACOS_CORE_VERSION`.
`BLAZER_MACOS_SIGN_IDENTITY` выбирает сертификат подписи. Каталог приложения
хранится в `~/Library/Application Support/Blazer`, ключ хранится в Keychain.
Проверка подписи, доступа к Keychain и сетевого прогона выполняется на Mac.

`scripts/build-apple.sh macos-tunnel` добавляет системное VPN-расширение в
`Blazer.app/Contents/Library/SystemExtensions`. Укажите
`BLAZER_MACOS_TUNNEL_CORE_LIBRARY` (статическая библиотека BlazeCore с feature `ffi`),
`BLAZER_MACOS_TUNNEL_CORE_HEADER` и `BLAZER_MACOS_TUNNEL_CORE_SHA256`. Пакет требует
Developer ID Application в `BLAZER_MACOS_SIGN_IDENTITY` и два provisioning profile
одной команды Apple для `com.kobaproduction.blazer` и
`com.kobaproduction.blazer.packet-tunnel` с entitlement
`packet-tunnel-provider-systemextension`. Пути задаются через
`BLAZER_MACOS_APP_PROFILE` и `BLAZER_MACOS_TUNNEL_PROFILE`. Для приёмки переместите
подписанный `Blazer.app` в `/Applications`, подтвердите активацию расширения
и проверьте маршруты, DNS, трафик, отключение и повторное подключение.
[Поставка Network Extension](https://developer.apple.com/documentation/technotes/tn3134-network-extension-provider-deployment) ·
[Активация системных расширений](https://developer.apple.com/documentation/systemextensions/installing-system-extensions-and-drivers/)

Аргумент `ios-simulator` собирает `Blazer.app` для симулятора.
Для симулятора Apple Silicon нужен `aarch64-apple-ios-sim`,
для Intel — `x86_64-apple-ios`. Установите bundle через `xcrun simctl install`
на запущенном симуляторе и откройте `com.kobaproduction.blazer`.
iOS использует общий Rust-каталог и импорт с ключом в Keychain.
Обе оболочки прошли `cargo check` для ARM64 из Windows без нативной сборки
расширения. Для физического iPhone сборка `ios-device` включает приложение и
`BlazerTunnel.appex`. Нужны `aarch64-apple-ios`, Apple signing identity
(`BLAZER_IOS_SIGN_IDENTITY`) и два provisioning profile с `packet-tunnel-provider`
(`BLAZER_IOS_APP_PROFILE` и `BLAZER_IOS_TUNNEL_PROFILE`). Статическую библиотеку
BlazeCore, соответствующий заголовок и проверенный SHA-256 задают через
`BLAZER_IOS_CORE_LIBRARY`, `BLAZER_IOS_CORE_HEADER` и `BLAZER_IOS_CORE_SHA256`.

На iPhone приёмка проверяет согласие на VPN, передачу TCP, UDP и DNS,
переход в фон, смену Wi‑Fi и сотовой сети, отключение и повторное подключение.
Подписанный пакет и эти сценарии требуют Mac с Xcode и физического устройства.

Домен и application не привязаны к платформе. Ядро клиента используют desktop,
Android и headless. Headless dependency graph не включает GUI/window/webview библиотеки.
Текущий `blazer --cli status` — завершающийся запрос, не daemon, IPC-сервис или
подтверждённая сборка для OpenWrt. Проверки разработки выполняются локально.

Системные API и особенности VPN-туннеля принадлежат внешним адаптерам с
целевыми зависимостями. Не устанавливать Windows target глобальным Cargo default.
Для сборки на Windows закреплён host toolchain MSVC. Android собирается с
явным Rust target и NDK. Скрипт `scripts/build-android.ps1` компилирует JNI-модуль
и нативную Activity с общим egui-интерфейсом и собирает APK. Android хранит ключ каталога в Keystore
и блокирует одновременную запись в каталог через POSIX `fcntl`.
HTTP-загрузка использует общий адаптер, а проверка HTTPS на Android опирается
на rustls с корневыми сертификатами WebPKI.

В Windows Blazer передаёт профиль и правила маршрутизации обычному процессу
BlazeCore. Тот подключается к службе BlazeCoreTun, которая владеет Wintun,
маршрутами и DNS на время сеанса. Клиент публикует состояние подключения
после сигнала готовности ядра. При завершении процесса служба освобождает
маршруты и адаптер. Для подключения используется обычный сеанс Windows.

Windows Setup объединяет клиент, ядро, Wintun и управление службой для SID
пользователя. Новый lifecycle установщика требует отдельной приёмки в Windows VM;
наличие кода и сборка Setup не доказывают установку/обновление SCM на машине.
Ручной контракт ядра: [BlazeCore](https://github.com/ArthurKoba/BlazeCore/blob/master/docs/platforms.md#windows-tun-service).
Локальный SOCKS5/HTTP-прокси доступен независимо от службы.

Отдельный тест локального прокси после повторного чтения каталога подтвердил
TLS ALPN `http/1.1` и передачу байтов для Xray JSON, sing-box JSON в Base64 и URI.
Для XHTTP проверена передача байтов через локальный BlazeCore по HTTP/1.1 в четырёх
режимах. HTTP/2 и HTTP/3 проверены в режиме `stream-one` после перезапуска
headless-клиента и повторного чтения каталога.
На Windows импорт Xray `extra` проверен в режиме `packet-up` с пользовательскими
ключами XHTTP. Стенд получил точные байты в двух обменах по одному соединению
для комбинаций размещения `header/query`, `cookie/header` и `query/cookie`.
WebSocket Early Data проверен через локальный прокси и закреплённый BlazeCore.
В тесте URI Xray отправил VLESS-заголовок в `Sec-WebSocket-Protocol`. Профили
sing-box в Base64 отправили его в собственном HTTP-заголовке и в суффиксе пути.
Стенд проверил фактический запрос и ответные данные во всех трёх сценариях.
Импорт Xray VLESS с `encryption` прошёл локальный обмен точными байтами
через BlazeCore для TCP с `native/0rtt` и WebSocket с Vision и `xorpub/1rtt`.
Hysteria2 прошёл IPv4/TCP-обмен через QUIC между двумя экземплярами BlazeCore
после импорта URI и sing-box JSON в Base64 и повторного чтения каталога.
URI с правильным SHA-256 сертификата установил соединение, а с неверным
отпечатком передача данных была отклонена.
Локальный SOCKS5-сервер получил точные учётные данные и TCP-байты в трёх
прогонах после импорта. Проверены Base64 пары учётных данных, Base64 всей
SOCKS5-ссылки без схемы и массив с обёрнутым sing-box outbound в Base64.
При отдельном прогоне release-сборки принудительно завершили headless-клиент
с активным локальным прокси. Windows остановила принадлежащий клиенту BlazeCore,
и его loopback-порт освободился.

Для проверки интеграции в обычном Windows-сеансе с уже запущенной службой
задайте BLAZER_TEST_BLAZECORE_EXE с абсолютным путём к закреплённому
blazecore.exe. Тест подключает маршрут только к 198.51.100.213/32,
передаёт TCP-байты через Wintun и локальный SOCKS5 и завершает процесс ядра.

```powershell
cargo test --locked --target x86_64-pc-windows-msvc -p vpn-infrastructure client_engine::windows_tunnel::tests::managed_tun_sends_selected_traffic_and_stops_under_blazer_ownership -- --ignored --exact
```

На эмуляторе API 35 x86_64 обычный запуск открыл общий egui-интерфейс.
Ввод в форму сохранился при переходе с узкой на среднюю компоновку после поворота.
Из интерфейса проверены импорт профиля и загрузка подписки Base64 с локального стенда.
С библиотекой BlazeCore системный диалог открыл TUN, TCP-обмен продолжался
после ухода Activity в фон. После отключения интерфейс TUN был снят.
На том же API ранее проверены установка APK, запуск,
импорт ссылки, загрузка подписки с sing-box JSON в URL-safe Base64, обновление
через Xray JSON, выбор нового профиля и восстановление выбора из зашифрованного
каталога после перезапуска процесса. Прогон выполнен после переноса Android-адаптеров
в инфраструктуру и разделения разбора форматов и проверки совместимости с движком.
Отдельно проверено автоматическое обновление Happ после запуска нового процесса.
Локальный сервер получил ровно два запроса, при добавлении и при перезапуске.
В сценарии с прерыванием сервер получил исходный запрос, прерванный запрос
при открытии и один повторный запрос. Смена языка завершилась до ответа сервера,
после чего приложение приняло обновлённую подписку.
Системное обновление проверено на том же эмуляторе после завершения процесса.
Принудительный запуск задания до срока не вызвал сетевых запросов. После сдвига
часов эмулятора за срок обновления задание загрузило подписку один раз, запланировало
следующий запуск и не вывело Activity на передний план. Режимы сна и перезагрузка
устройства требуют отдельных проверок.
Отдельный прогон получил `503` после наступления срока. После завершения
процесса досрочный запуск задания не отправил HTTP-запрос. Когда интервал истёк,
задание выполнило одну загрузку, и клиент восстановил обновлённый профиль при открытии.
В проверке с `ETag` исходная загрузка передала 41 байт профиля. Обновление
при открытии и системное задание после завершения процесса отправили
`If-None-Match` и получили два ответа `304` с нулём байт тела.
Сборка содержит ARM64 ABI, проверка на ARM64-устройстве ещё предстоит.
В APK с библиотекой BlazeCore доступен Android-адаптер `VpnService`. Он запрашивает
системное согласие и запускает foreground-службу на время соединения. При
открытии сессии адаптер проверяет наличие `libblazecore.so` в каталоге установленных
библиотек приложения. Версия и инициализация BlazeCore проверяются на worker при
первом подключении, до открытия TUN. Импорт и обновление подписок используют
ядро клиента на Rust без загрузки BlazeCore. Маршруты
IPv4 и IPv6 принадлежат ОС; клиент использует DNS BlazeCore через выбранный прокси.
В текущей конфигурации адрес DNS-резолвера фиксирован (`1.1.1.1`).
На эмуляторе API 35 x86_64 получено системное согласие и подтверждён обмен
точными байтами через TUN, BlazeCore и локальный HTTP CONNECT. После отключения
тестовый ответ через этот маршрут не получался. Этот тест покрывает IPv4/TCP.
В дополнительном прогоне с TLS-профилем, импортированным из URI, локальный
сервер согласовал ALPN `http/1.1` и получил подтверждение приёма точного ответа
клиентом через TUN.
VLESS XHTTP `stream-one` по HTTP/2 также проверен через системный Android TUN
после импорта URI. Приложение получило точный ответ и штатно отключилось.
Для VLESS Vision с `encryption` отдельный прогон API 35 x86_64 проверил URI,
IPv4/TCP через системный TUN и BlazeCore с `native/1rtt` и ML-KEM-768 для
аутентификации сервера. Удалённая сторона получила подтверждение точного ответа,
а после отключения пробный адрес стал недоступен.
Hysteria2 URI с закреплённым сертификатом также прошёл IPv4/TCP через
системный Android TUN и QUIC до BlazeCore. После штатного отключения тестовый
адрес стал недоступен.
В отдельном прогоне тот же APK передал обычную IPv4 UDP-датаграмму с точным
ответом через Android TUN и Hysteria2 по QUIC. Синтетический адрес был
недоступен до подключения и после отключения.
На API 35 x86_64 APK также передал UDP с Salamander через системный TUN.
Стенд использовал отдельную реализацию формата пакетов Salamander и сервер BlazeCore.
На API 26 x86_64 проверен также VLESS WebSocket Early Data через системный
Android TUN. Стенд получил VLESS-заголовок в `Sec-WebSocket-Protocol`, сверил
двусторонний TCP-обмен и подтвердил восстановление маршрута после отключения.
На эмуляторах API 26 и API 35 x86_64 проверен отказ запуска VPN-службы без
профиля и последующее подключение после импорта. Повторный запуск службы
сохранил активный туннель. Точный ответ по UDP получен до и после повторного
запуска. После отключения тестовый адрес снова стал недоступен.
На API 35 x86_64 обновление заменило обёрнутый профиль sing-box на обёрнутый
профиль Xray. Оба ответа также содержали Base64 SOCKS5-ссылки. Выбор профиля
восстановился из каталога после перезапуска. Отдельный прогон передал TCP
через системный TUN с профилем из полного Base64. SOCKS5-сервер сверил
исходные учётные данные и байты.
Формат каталога и проверки импортируемых профилей общие для Windows и Android.

22 ноября 2026 года Android 15 x86_64 прошёл leastPing через настоящий TUN/JNI
с ядром из исходного commit `10512be`: A → B → fallback C → A, сохранение
открытого потока, остановка и отклонение устаревшего handle после перезапуска.
Клиент использует общий Rust-наблюдатель, а не отдельную Android-политику.
Доверенный набор содержит закреплённые `x86_64` и `arm64-v8a` библиотеки JNI 0.3.7;
ARM64 проверен сборкой и ELF/экспортами, не сетевым запуском на устройстве.
Прежние Android-запреты расширенного XHTTP и Native-профилей удалены: применяется
общий строгий валидатор, а leastPing дополнительно требует selector capability.
Тесты подтверждают сохранение XHTTP metadata/packet wire и Native VLESS/WireGuard
в конфигурации; это не новый сетевой прогон этих протоколов.
SHA-256 артефактов и воспроизводимый сценарий: [приёмка балансировщиков](xray-balancer-acceptance.md).

Переносимость исходников не доказывает GUI, VPN API, фоновые службы, ABI,
маршрутизацию, DNS или packaging на другой ОС. Текущая Windows GUI/headless
сборка не доказывает Windows VPN: [core platforms](https://github.com/ArthurKoba/BlazeCore/blob/master/docs/platforms.md)
владеют статусом модифицированного ядра. Инсталлятор и подпись релиза пока
не настроены. Мобильное выполнение внешних модулей зависит также от store policy;
целевые trust-критерии находятся в [product requirements](product-requirements.md).
