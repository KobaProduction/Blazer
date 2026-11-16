# Локальная разработка и проверки

Версии Rust/rustfmt/Clippy задаёт `rust-toolchain.toml`, зависимости — `Cargo.lock`.
На свежем checkout сначала явно установить MSVC toolchain по README.
Все проверки используют `--locked`. Собственный код — MIT; manifest каждого
пакета наследует лицензию. Лицензии dependencies остаются своими: перед
доставкой артефакта сохранять применимые upstream notices и разрешения,
а не считать весь dependency graph автоматически MIT.

Когнитивная сложность функций максимум **15**, порог в `clippy.toml`,
уровень `forbid` во всех пакетах. Не повышать порог и не подавлять правило.
`unsafe_code` запрещён в собственном коде; это не утверждение об отсутствии
unsafe внутри third-party GUI/SDK dependencies.

## Цикл разработки

Пакетировать изменения и проверять только затронутое поведение. Например,
для import guard запускать `cargo test --locked -p vpn-application`, для
backend/diagnostics — соответствующий пакет. Не повторять успешные проверки
без изменения, сбоя или новой причины. После стабилизации выполнить один
полный финальный gate перед commit/отправкой изменений.

По явному решению владельца CI не используется: не запускать, не ждать
и не принимать его как критерий разработки. Это исключение из CI-default
[Project Bootstrap](https://github.com/ArthurKoba/ai-agent-workflow/blob/main/skills/project-bootstrap.md).
Автоматический GitHub workflow удалён. Локальный pre-push остаётся обязательным.

## Команды и gate

`scripts/verify.ps1` выполняет архитектуру, format, Clippy, type-check, tests
и release-build обычного клиента и headless для Windows MSVC. Проверки
Clippy, type-check и tests включают также демо. Параметр `-Stage` выбирает
этап `Architecture`, `Format`, `Lint`, `Check`, `Test` или `Build`.
Локальные папки с исполняемыми файлами клиента и демо создаёт
[`scripts/build-windows.ps1`](../scripts/build-windows.ps1).
`cargo lint` и `cargo typecheck` — короткие host-native команды.

Для разработки используйте обычный Debug и постоянные папки
`dist/windows/client` и `dist/windows/demo`. Указывайте
`-Configuration Release` для оптимизированного пакета. Оба профиля
переиспользуют общую папку `target` с отдельными кешами профилей и целей.
В dev и test сохраняются номера строк для backtrace собственного кода,
отключены incremental и отладочная информация зависимостей. Это ограничивает
рост локального кеша ценой более долгой повторной компиляции изменённых пакетов.
[Профили Cargo и скорость сборки](https://doc.rust-lang.org/cargo/guide/build-performance.html)

Windows-скрипт заменяет только файлы пакета и переиспользует уже проверенные
`shoes.exe` и `wintun.dll`, если их SHA‑256 совпадает. Запустите один
экземпляр упаковщика за раз. Если Windows удерживает exe, закройте
соответствующий клиент или демо и повторите ту же команду. Для Android
используйте `scripts/build-android.ps1` и сохраняйте тот же путь к APK.
Скрипт заменяет APK после проверки подписи и удаляет временные файлы упаковки.

Architecture gate контролирует направление зависимостей, MIT/lint inheritance
и GUI-free headless dependency tree. Полные тесты включают 1 MiB guard,
explicit import errors, engine absence, bounded ring/drop counter,
snapshot без новых событий и отказ экспорта. Headless subprocess проверяется
с OTEL env, указывающими на local test listener: соединений быть не должно.
Это не тест настоящего VPN data path.

## Локальные hooks и публикация

После clone установить `git config --local core.hooksPath .githooks`.
`.githooks/pre-push` должен быть executable и запускать тот же local gate.
Ошибка блокирует push; не использовать `--no-verify`.
Credentials/подписки/ключи не должны попадать в Git, логи или артефакты.

GUI/runtime и performance evidence собираются отдельно от build/test.
Методика измерений: [performance](performance.md). Не называть заглушку
Normalizer/Engine реализованным importer или VPN и не выдавать parity,
мобильную поддержку либо экономию энергии без отдельных доказательств.
