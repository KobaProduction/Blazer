# Референс Happ и отличия Blazer

Дата сбора: 11 ноября 2026 года. Пользователь назвал референс «Hub/HAP/Хап»;
по установленному приложению и описанному Xray/subscription поведению это
соотнесено с **Happ — Proxy Utility**. Основные пользовательские сценарии
воспроизводятся самостоятельной реализацией. Требования принадлежат
[Blazer](product-requirements.md).

## Отличия Blazer

- В целевой архитектуре один режим TUN обеспечивает BlazeCore. Платформенный
  адаптер предоставляет интерфейс ОС. Список реализаций TUN в Happ служит материалом для
  сопоставления.
- При изменении маршрутизации активного соединения Blazer предлагает
  применить правила сейчас с переподключением либо при следующем подключении.
  Закрытие окна откладывает применение. В обоих случаях изменения сохраняются.

## Происхождение фактов

- Локальный Windows inventory: установлен `Happ - Proxy Utility 4.1.3(604)`,
  publisher `Happ`; executable metadata: ProductName `Happ`,
  ProductVersion/FileVersion `4.1.3.604`, CompanyName `FlyFrog LLC`.
  Запись установки указывает `https://happ.su`.
- [Официальный сайт](https://happ.info/) описывает Happ как Xray-based клиент.
  Это не доказательство GUI-технологии установленного binary.
- 16 ноября Analyzer R открыл `Happ.exe` из официального пакета 4.1.3.
  SHA-256 совпал с установленным EXE
  `c6c5c2b22c2794d39a7d8a9a87ccfab30b60865481bf2e599e6a56fc98f8deef`.
  PE-импорты показывают Qt 6 Network, SQL, QML и Quick. Анализ ссылок
  подтвердил `SubscriptionModel`, SQLite `subs.db`, классификацию директив
  подписки и отдельную обработку защищённой ссылки и `encrypt-tag`.
  В открытом Happ просмотрены каталог, меню подписки, журнал и настройки
  подписок. Сетевые действия и настройки Happ в этом проходе не менялись.
- Unbihex идентифицирован по [публичному репозиторию crystalmasta/unbihex](https://github.com/crystalmasta/unbihex/tree/7235759c84488af1fc2845d7d52ce51dac81fb0f).
  Его отдельные факты и ограничения приведены ниже; это не установленный Happ.

Маркировка: **D** — описано официальной документацией; **I** — подтверждена
только локальная identity; **U** — не наблюдалось/не проверено; **T** — требование
нашего продукта из [product-requirements](product-requirements.md).
D не означает runtime-совместимость нашего клиента и не заменяет I/U.

## Матрица действий

| Область / действие | Доказательство референса | Цель и проверка нашего клиента |
| --- | --- | --- |
| Одна основная кнопка, выбрать сервер, подключить/отключить | U; T по владельцу | C01: реальные состояния, отмена, повторное действие, ошибка |
| Ручной ввод конфигурации через добавление и список серверов | D: [добавление](https://www.happ.su/main/faq/adding-configuration-subscription) | C02/C05: валидация и сохранение |
| URI VLESS, VMess, SOCKS, Trojan, Shadowsocks, Hysteria2 | D: [поддержанные формы](https://www.happ.su/main/faq/adding-configuration-subscription) | C03/C04: fixtures отдельно от engine capabilities |
| Импорт из буфера, QR, deep link | D: [способы импорта](https://www.happ.su/main/faq/adding-configuration-subscription) | C03: одинаковый результат нормализации |
| Обычная web-подписка | D: [подписки](https://www.happ.su/main/faq/adding-configuration-subscription) | C02/C04: безопасное обновление, ошибки, last-known-good |
| Криптоссылки и скрытые конфигурации | D: [crypto link](https://www.happ.su/main/dev-docs/crypto-link.md) | Отдельная compatibility граница; закрытые форматы не объявлять поддержанными |
| Ограниченные/HWID/шифрованное содержимое | D: наличие тем в [индексе](https://www.happ.su/main/dev-docs); детали U | Получить спецификацию/разрешённые fixtures; отказ с понятной причиной до реализации |
| Редактировать/удалить/переупорядочить серверы и подписки | U для установленной версии; T | C02/C05: persist, независимость профилей, безопасное удаление |
| Поделиться сервером через буфер, QR, JSON | D: [share configuration](https://www.happ.su/main/faq/share-configuration.md) | C10: round-trip и подтверждение раскрытия секрета |
| Проверить подписку, отдельный сервер, текущее соединение | D: [ping](https://www.happ.su/main/faq/ping.md) | C09: ограниченные пробы, timeout и cancellation |
| Методы и отображение ping, проверки при запуске | D: [ping](https://www.happ.su/main/faq/ping.md) | C09/C15: отличать метод/доступность, показывать стоимость фоновых проб |
| Routing profiles по подписке, import/update и DNS/geo rules | D: [routing](https://www.happ.su/main/dev-docs/routing) | C06: typed validation и независимость профилей |
| Применение routing после reconnect, атомарная смена geo-набора | D: [routing lifecycle](https://www.happ.su/main/dev-docs/routing) | C06/C07: отличать current/pending, сохранять рабочую версию |
| Особые ограничения JSON routing и tunnel DNS | D: [JSON mode](https://www.happ.su/main/dev-docs/routing) | Проверить совместимость и отличия UX отдельно; не менять provider JSON незаметно |
| Управление через subscription metadata | D: [app management](https://www.happ.su/main/dev-docs/app-management) | Ниже сгруппирован аудит; недоверенный импорт не получает системных полномочий |
| Настройки VPN и logging как самостоятельные экраны | U для локальной версии; T | C07/C08: capability-aware UI и ограниченная диагностика |
| Сворачивание/фон/tray/автозапуск | U для конкретной версии | C07/C11/C15: lifecycle и энергия до обещания паритета |
| Внешний вид, темы, accessibility, навигация | U для конкретной версии; T: простой native UI | Проверить размеры/touch/keyboard без копирования бренда |
| Device/session control, BlazeCore modules, OTel | Не установлено как Happ parity; наши T | Отдельные требования продукта и core authority |

Официальная страница app management содержит много platform/provider-specific
параметров. Группы ниже сохраняют охват исследования, но не подменяют тесты
каждого параметра; все перечисленные группы — D по
[официальному описанию](https://www.happ.su/main/dev-docs/app-management), локально U:

| Группа параметров | Включённые темы для отдельного аудита |
| --- | --- |
| Subscription metadata | Имя, traffic/expiry, ссылки, объявления, описание, уведомления |
| Обновление подписки | Interval, launch, новый/резервный URL и домен, timeout, User-Agent |
| Provider controls | Скрытие, HWID, pin/collapse/filter/sort и запреты изменения |
| Сеть | DNS, fragmentation/fronting/noises, sniffing, mux |
| Lifecycle/проверки | Autoconnect, autostart, autoping, способ и вид ping |
| VPN/proxy | HTTP/SOCKS auth, proxy/TUN, выбор/конфигурация tunnel, MTU, excludes |
| Per-app routing | Include/exclude/invert и desktop/mobile варианты |
| Platform UI/options | iOS themes/networks/APNS, proxy icon, Android tunnel binding |

Ограничения и trust-последствия provider controls оцениваются отдельно:
функциональный паритет не означает автоматическую отправку HWID, принятие
любого URL или удалённое выключение пользовательской защиты.

В клиенте поддержано расписание по `profile-update-interval` в часах. При
старте процесса проверяются пропущенные сроки и флаг
`subscription-auto-update-open-enable` в пределах подписки без Provider ID.
Область действий поставщика и поведение при ошибках описаны в [архитектуре](architecture.md).
Исключение подтверждённого пользовательского сценария из объёма требует
явного решения владельца.

## Подписки Windows 4.1.3.604

В каталоге Happ строка подписки показывает последнее обновление и интервал.
Карточка содержит отдельные область информационного сообщения и кнопку,
строку трафика, ссылки и обычное объявление. Меню ведёт в маршрутизацию,
обновление и проверку пинга, поддерживает закрепление, редактирование и
удаление. Общие настройки открываются через «Настройки → Подписки».
Там находятся интервал, таймаут, уведомление об обновлении, действия при
запуске, сворачивание, подавление дублей и фильтр. У журнала есть переход через
боковую панель. Основная вкладка содержит и события подписок. Дополнительно
доступны вкладки подписок, ядра, туннеля, AntiFilter, службы и пушей.
Отдельные настройки каждой подписки заявлены в более позднем
[релизе 4.4.6](https://github.com/Happ-proxy/happ-desktop/releases/tag/4.4.6).

В Analyzer R ссылки из `0x1400234b0` ведут к `announce`,
`profile-update-interval`, `ping-type`, `sub-info-text` и
`subscriptions-sort-type`. Qt-методы модели различают
`subscriptionParamsUpdate`, `subscriptionStateUpdate` и
`subscriptionUpdatedDone`. В каталоге Blazer состояние подписки и статусы
операций тоже проходят через снимок состояния ядра. Кнопки перехода используют
проверенные HTTP(S)-ссылки и открываются по действию пользователя.

Blazer читает известные HTTP-заголовки и начальные комментарии текстовой
подписки, в том числе после Base64. Заголовки имеют приоритет. Сетевой
ответ обновляет собственную подписку. Provider ID не является условием
для её настроек. Неподдерживаемое поведение сохраняется в метаданных без
автоматического применения. Точный список имён и регистр сопоставления
задаёт [ProviderKey](../src/domain/provider.rs).

`ETag` и `If-None-Match` обслуживают условное обновление. `Cache-Control`
с `no-store` и `Vary: *` отключают сохранение версии. `Location` обрабатывается
по политике переходов HTTP, а код и доступное безопасное описание ответа
показываются при ошибке. `Content-Disposition` не определяет имя подписки
в Blazer. Произвольные неизвестные заголовки не меняют настройки.

| Директивы | Действие в Blazer |
| --- | --- |
| `subscription-userinfo`, `announce`, `support-url`, `profile-web-page-url` | Трафик и срок, обычное объявление, кнопки сайта и поддержки |
| `sub-info-text`, `sub-info-color`, `sub-info-button-text`, `sub-info-button-link`, `sub-expire`, `sub-expire-button-link` | Информационный баннер с кнопкой и приоритет предупреждения об истечении в трёхдневном окне |
| `subscription-pin`, `subscriptions-collapse`, `subscriptions-expand-now`, `subscriptions-sort-type`, `ping-result` | Закрепление, доступность свертывания, раскрытие при обновлении, порядок серверов и представление пинга конкретной подписки |
| `profile-update-interval`, `subscription-auto-update-open-enable`, `subscription-request-timeout` | Интервал, обновление при запуске и ограниченный таймаут запроса |
| `fallback-url`, `new-url`, `new-domain`, `hide-settings` | Резервный источник, миграция с проверкой origin и скрытие сведений профиля |
| `profile-title`, `name` | Определяют название при добавлении и обновлении подписки с автоматическим именем. При отсутствии этих полей клиент проверяет параметр `name` в URL, затем использует имя хоста. Пустое поле имени включает этот режим. Имя, заданное вручную, сохраняется |
| `providerid`, `subscription-auto-update-enable`, `change-user-agent`, `manual-block-user-agent`, `dont-use-filter` | Проводные значения распознаются. Глобальные настройки и фильтры сохраняют существующее поведение |
| `notification-subs-expire`, `subscription-ping-onopen-enabled`, `ping-type`, `check-url-via-proxy`, `proxy-ping-mode`, `proxy-ping-timeout` | Будущие отдельные сценарии системного уведомления, автопинга и измерения через BlazeCore. Текущий прямой TCP-пинг использует ядро клиента |
| `color-profile`, `subscription-autoconnect*`, `app-auto-start`, `subscription-*-hwid-*`, `no-limit-*` | Документированные параметры Happ без применения к настройкам и полномочиям приложения Blazer |
| `routing*`, `*-auth-*`, `custom-tunnel-config`, `tun-*`, `proxy-enable`, `xray-tun-*`, `inbound-http-enable`, `exclude-routes*`, `include-all-networks-enable`, `exclude-*-enable`, `per-app-proxy-*`, `block-bind-to-tunnel-enable`, `hide-vpn-icon` | Платформенные настройки Happ. Реальные операции туннеля и маршрутизации Blazer используют собственные типизированные команды |
| `fragmentation-*`, `noises-*`, `mux-*`, `sniffing-enable`, `server-address-resolve-*`, `dns-from-json-enable`, `user-agent-geo-files` | Параметры сетевого ядра Happ. Совместимость определяется возможностями BlazeCore |
| `encrypt-tag`, `happ://crypt4/`, `happ://crypt5/` | Отдельные защищённые форматы отложены |

Сводный и подписочный журнал Blazer получают результат обновления, сбой и
использование fallback без адресов подписок. Ошибка запроса остаётся привязана
к карточке подписки. Уведомления внутри окна включаются через «Настройки →
Подписки» на время текущего сеанса. Предупреждение об истечении пересчитывается
при публикации нового снимка. [Формат Happ](https://www.happ.su/main/dev-docs/app-management)

## Дополнительный референс Unbihex

Исследована ревизия `7235759c84488af1fc2845d7d52ce51dac81fb0f` публичного
репозитория, без клонирования, запуска или копирования кода. Уточнение к
исходному описанию владельца: исходники доступны, а подписки уже описаны.
Слово «открытый» здесь не означает MIT или разрешённое заимствование.

| Первичный источник | Подтверждённый факт и предел вывода |
| --- | --- |
| [README](https://github.com/crystalmasta/unbihex/blob/7235759c84488af1fc2845d7d52ce51dac81fb0f/README.md) | Описаны собственный Rust engine, client и test server, desktop/Android и Linux headless CLI/TUI. Заявления автора о платформах не являются нашими runtime-проверками |
| [Client README](https://github.com/crystalmasta/unbihex/blob/7235759c84488af1fc2845d7d52ce51dac81fb0f/unbihex-client/README.md) | Описаны HTTPS-подписки plain/base64, metadata и обновление; Rust-логика отделена от HTML/JavaScript UI на Tauri 2. Этот UI-стек не принимается в нашем продукте |
| [Engine SPEC](https://github.com/crystalmasta/unbihex/blob/7235759c84488af1fc2845d7d52ce51dac81fb0f/unbihex-engine/SPEC.md) | Авторский контракт proxy/TUN/routing и platform embedding; не независимое доказательство interop или энергопотребления |
| [Test-server README](https://github.com/crystalmasta/unbihex/blob/7235759c84488af1fc2845d7d52ce51dac81fb0f/test_server/README.md) | Тестовый VLESS/REALITY peer, явно не production server: не основание обещать management API, live user/session control или production security |
| [LICENSE](https://github.com/crystalmasta/unbihex/blob/7235759c84488af1fc2845d7d52ce51dac81fb0f/LICENSE) | BSL 1.1 source-available с условиями production-use и последующей сменой на MPL 2.0 по указанному в лицензии правилу; текущая версия не MIT и не заявляется Open Source |
| [Авторский interop-отчёт](https://github.com/crystalmasta/unbihex/blob/7235759c84488af1fc2845d7d52ce51dac81fb0f/test-reports/2026-11-04-01.md) | Содержит passes/failures/ограничения старой ревизии `f66f294`; не battery/power benchmark текущей версии |

Полезные идеи для самостоятельного проектирования: отдельное ядро и client
application, headless entrypoints, platform hooks и явная диагностика
неподдержанных возможностей. Эти идеи не являются доказательством реализации
аналогичных функций у нас или замены выбранного BlazeCore.

**Граница владельца: наш код — только MIT, из Unbihex берём только концепции
и идеи, никакой код не переносится.** Его BSL не становится лицензией или
зависимостью нашего продукта. Экономичность требует нашего сравнимого baseline;
изученные документы и отчёт не доказывают превосходства по энергии.

## Как проверять выбранные сценарии

Для выбранной эталонной версии составляется inventory всех доступных меню,
действий, настроек, platform-only и provider-only вариантов. Каждая строка
разворачивается в стабильный сценарий: preconditions, действие, наблюдаемый
результат, ошибка/отмена, persistence, side effects, resource cost.
Покрытие относится к версии и платформе, не ко всему имени Happ навсегда.

Обязательный набор определяется требованиями Blazer. Результаты исполнения
и задачи хранятся в Issues и свидетельствах тестов. Здесь фиксируются действия
референса и существенные отличия собственной реализации.

При исследовании не включать VPN, не менять маршруты/DNS, consent или telemetry
и не выгружать реальные подписки. Пользовательские secrets не нужны для fixtures.
Сначала наблюдаемое поведение и официальные описания; инструменты реверса не
устанавливаются без конкретной технической необходимости. Закрытый код,
секреты, artwork и лицензированные ресурсы не переносятся в MIT-проект.
