# Архитектурный контракт клиента

Продуктовые обязательства: [требования](product-requirements.md);
референс и границы паритета: [Happ](reference-behavior.md).
Справочник: [AI Agent Workflow / Clean Architecture / DDD](https://github.com/ArthurKoba/ai-agent-workflow/blob/main/skills/software-engineering/clean-architecture-ddd.md).
Требование в документации не означает реализованный VPN.

## Границы пакетов

| Пакет | Ответственность | Разрешённые внутренние зависимости |
| --- | --- | --- |
| `vpn-domain` | Предметные значения и инварианты | Нет |
| `vpn-application` | Сценарии, import guard, engine/normalizer ports, типизированные ошибки | `vpn-domain` |
| `vpn-infrastructure` | Реализации внешних портов | `vpn-application`, `vpn-domain` |
| `vpn-backend` | Общая composition root, состояние, facade, OTel/diagnostics | `vpn-infrastructure`, `vpn-application`, `vpn-domain` |
| `vpn-client` | Нативный GUI и presentation state | `vpn-backend` |
| `vpn-headless` | Консольная доставка без окна | `vpn-backend` |

Домен не имеет внешних зависимостей; application зависит только от домена.
GUI, OS API, SDK и конкретные реализации портов находятся во внешних слоях.
Та же граница действует для test/build dependencies. Dependency gate проверяет
шесть пакетов, наследование lint-правил, MIT собственных пакетов и отсутствие
GUI/window/webview библиотек в headless-графе.

Mutable состояние принадлежит backend. GUI хранит screen enum и cached snapshot;
отрисовка не запрашивает engine status и не генерирует telemetry.
Обновление статуса происходит при запуске и явном действии пользователя.
Не добавлять постоянный repaint/poll timer. Будущие I/O-адаптеры требуют
отменяемого фонового исполнения; нельзя блокировать GUI-поток.

## Текущий исполняемый контракт

Оба entrypoint используют один backend с явно недоступными engine и normalizer.
`vpn-headless status` выводит реальное состояние этих адаптеров и завершается;
это не daemon, системная служба или установленный VPN. GUI имеет экраны
подключения, подписок, конфигураций, маршрутизации, VPN, журнала и телеметрии,
но незавершённые функции обозначены как недоступные. Кнопка подключения крупная
и отключена: рабочий engine/data path не реализован.

## Подписки и конфигурации

Входной документ имеет формат и bytes; размер ограничен 1 MiB до вызова
normalizer. Декларированы Xray JSON и список share links как форматы будущего
ingestion, а не реализованные parsers. Native format не заявлен без parser.

Цепочка: input document → guarded application import → normalizer port →
typed normalized configuration → engine validation. Модель не является
`serde_json::Value` или произвольным Xray JSON. Текущий normalizer всегда
возвращает `NormalizerUnavailable`; неподдержанный формат/feature и отсутствие
engine имеют отдельные ошибки. Никакого parse-success или активного профиля
из сырых bytes не создаётся. Downloader, translator, persistence, provider
metadata, QR и закрытые форматы пока не реализованы.

Небольшой protocol enum — словарь model contract, не feature matrix поддержки.
Реальная нормализованная схема расширяется под fixtures и версионированный
core contract, с отказом от неподдержанных полей без silent drop.

## Собственное модифицированное ядро

Источник ядра — [ArthurKoba/shoes](https://github.com/ArthurKoba/shoes),
его [архитектура](https://github.com/ArthurKoba/shoes/blob/master/docs/architecture.md)
владеет engine runtime/Device/Session и core/app contract. Не копировать его
schema в клиент. Library/FFI/process boundary выбирается по согласованному
versioned contract и платформенным ограничениям; сейчас Shoes dependency нет.

Engine вызывается только через application ports. Наличие процесса ядра
не доказывает VPN. Windows VPN runtime не заявляется до подтверждённой
поддержки со стороны ядра и реального data path. Privileged OS операции,
routes/DNS и service/IPC потребуют отдельных adapters и cleanup-контрактов.

## Нативный GUI и ресурсы

Сохраняем [egui/eframe](https://docs.rs/eframe/0.37.0/eframe/) + Glow:
Rust UI без JS, Electron и WebView, уже проверяемый Windows build.
Iced — подходящая native Rust модель messages/tasks, но смена toolkit сейчас
не даёт доказанной экономии. Slint добавляет отдельный UI DSL и требует
своей проверки лицензии/platform delivery. Оба остаются альтернативами,
не установленными зависимостями.

Toolkit — внешняя деталь; core/application/backend не зависят от layout.
Desktop выбор не доказывает Android/iOS GUI. Headless предназначен для будущего
OpenWrt/router deployment, но target/ABI/libc и runtime ещё не приняты.
Ресурсы измеряются по [performance contract](performance.md); Rust сам по себе
не доказывает энергоэффективность.

## OpenTelemetry и диагностика

API/SDK 0.33.1 подключены только во внешнем backend с trace features.
Provider использует `Sampler::AlwaysOff` и `Resource::builder_empty` со
статическими service.name/version. Нет exporters, processors, фоновых workers,
global provider или чтения exporter/sampler/resource environment settings.
Lifecycle, status и import-result вызывают реальные span API с enum/staticcodes.
Это wiring, не готовый collector/export pipeline или все traces/metrics/logs.

Локальный ring хранит максимум 128 fixed-size enum events; старые вытесняются,
dropped-counter насыщается. Нет raw payload, URL, IP, profile names или secrets.
Cached snapshot не создаёт событий. Запрос включения экспорта возвращает
`NotConfigured`. Наличие OTEL environment variables не включает сеть.
Consent, проверенный exporter, metadata policy и cleanup/TTL нужны до экспорта.
