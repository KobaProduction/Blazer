# Референс Happ: факты, сценарии и границы паритета

Дата сбора: 11 ноября 2026 года. Пользователь назвал референс «Hub/HAP/Хап»;
по установленному приложению и описанному Xray/subscription поведению это
соотнесено с **Happ — Proxy Utility**. Цель — воспроизвести полезные действия
самостоятельной реализацией, без копирования закрытого кода или ресурсов.

## Происхождение фактов

- Локальный Windows inventory: установлен `Happ - Proxy Utility 4.1.3(604)`,
  publisher `Happ`; executable metadata: ProductName `Happ`,
  ProductVersion/FileVersion `4.1.3.604`, CompanyName `FlyFrog LLC`.
  Запись установки указывает `https://happ.su`.
- [Официальный сайт](https://happ.info/) описывает Happ как Xray-based клиент.
  Это не доказательство GUI-технологии установленного binary.
- Ни версия из реестра, ни общий сайт не подтверждают все действия локальной
  версии. UI inventory/ручные сценарии пока не получены: предыдущий
  Computer Use helper завершался таймаутом. Сетевые настройки и соединения
  референса при этом исследовании не менялись.
- «Unbihex» не идентифицирован по точному имени/URL. Не приписывать ему
  свойства другого проекта; наличие установленного AmneziaVPN этого не решает.

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
| Device/session control, Shoes modules, OTel | Не установлено как Happ parity; наши T | Отдельные требования продукта и core authority |

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
Исключение подтверждённого пользовательского сценария из объёма требует
явного решения владельца.

## Как доказывать полный паритет

Для выбранной эталонной версии составляется inventory всех доступных меню,
действий, настроек, platform-only и provider-only вариантов. Каждая строка
разворачивается в стабильный сценарий: preconditions, действие, наблюдаемый
результат, ошибка/отмена, persistence, side effects, resource cost.
Покрытие относится к версии и платформе, не ко всему имени Happ навсегда.

До такого inventory фраза «полностью повторяет Happ» недоказана. Эта матрица
явно отделяет уже найденные темы от неизвестного. Результаты исполнения и
задачи хранятся в Issues/свидетельствах тестов; здесь остаётся контракт сценариев.

При исследовании не включать VPN, не менять маршруты/DNS, consent или telemetry
и не выгружать реальные подписки. Пользовательские secrets не нужны для fixtures.
Сначала наблюдаемое поведение и официальные описания; инструменты реверса не
устанавливаются без конкретной технической необходимости. Закрытый код,
секреты, artwork и лицензированные ресурсы не переносятся в MIT-проект.
