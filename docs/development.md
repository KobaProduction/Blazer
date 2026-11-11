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
и release-build двух entrypoint для Windows MSVC. `-Stage` выбирает один этап:
`Architecture`, `Format`, `Lint`, `Check`, `Test`, `Build`.
`cargo lint` и `cargo typecheck` — короткие host-native команды.

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
