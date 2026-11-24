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
ядро клиента и диагностика — соответствующий пакет. Не повторять успешные проверки
без изменения, сбоя или новой причины. После стабилизации выполнить один
полный финальный gate перед commit/отправкой изменений.

Проверки выполняются локально, включая обязательный pre-push.
GitHub Actions служит для сборки и публикации Windows-выпусков. Пуш в `main`
обновляет prerelease `preview`, пуш тега `vX.Y` выпускает стабильную версию.
Выпуск включает NSIS Setup и парные проверяемые update metadata. Внутренний ZIP
сохраняется как источник закреплённого ядра следующей сборки, не как updater.
Compiler NSIS 3.13 загружается только по закреплённому SHA-256;
локально переиспользуется существующая установка. Контракт и приёмка:
[Windows installation](windows-installation.md).
Immutable Setup требует чистого checkout; стабильный выпуск дополнительно
требует точного тега. Для проверки незакоммиченной реализации используйте
`build-windows.ps1 -Configuration Release -DevelopmentInstaller`: артефакт
с текущим клиентом блокирует установку до UAC/SCM и не является релизом.

## Команды и gate

`scripts/verify.ps1` выполняет архитектуру, format, Clippy, type-check, tests
и release-build Blazer для Windows MSVC со всеми режимами запуска.
Параметр `-Stage` выбирает
этап `Architecture`, `Format`, `Lint`, `Check`, `Test` или `Build`.
Локальный пакет Blazer создаёт
[`scripts/build-windows.ps1`](../scripts/build-windows.ps1).
`cargo lint` и `cargo typecheck` — короткие host-native команды.

Для разработки используйте обычный Debug и постоянную папку
`dist/windows/client`. Указывайте
`-Configuration Release` для оптимизированного пакета. Оба профиля
переиспользуют общую папку `target` с отдельными кешами профилей и целей.
В dev и test сохраняются номера строк для backtrace собственного кода,
отключены incremental и отладочная информация зависимостей. Это ограничивает
рост локального кеша ценой более долгой повторной компиляции изменённых пакетов.
[Профили Cargo и скорость сборки](https://doc.rust-lang.org/cargo/guide/build-performance.html)

Windows-скрипт заменяет только файлы пакета и переиспользует уже проверенный
`blazecore.exe`, если его SHA‑256 совпадает. Запустите один
экземпляр упаковщика за раз. Если Windows удерживает exe, закройте
Blazer и повторите ту же команду. Для Android
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
