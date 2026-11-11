# Разработка и проверки

Версии Rust/rustfmt/Clippy задаёт `rust-toolchain.toml`. Все зависимости
фиксируются `Cargo.lock`; проверки и CI используют `--locked`.

Когнитивная сложность функций: максимум **15**, конфигурация в `clippy.toml`.
Правило `clippy::cognitive_complexity` имеет уровень `forbid` во всех пакетах;
Clippy завершает работу с ошибкой при превышении. Локальные `allow`/`expect`
не являются допустимым обходом. `unsafe_code` запрещён в коде проекта;
это не утверждение об отсутствии unsafe в сторонних GUI-зависимостях.

## Локальный цикл

Пакетировать связанные изменения. Во время разработки проверять только
затронутый пакет/поведение, например `cargo test --locked -p vpn-application`.
Не повторять успешные проверки без изменения кода, сбоя или новой причины.
Полный финальный gate выполняется один раз на стабилизированном результате
перед commit/отправкой; pre-push и CI независимо обеспечивают проверку публикации.

Отдельные этапы доступны через `scripts/verify.ps1 -Stage`:
`Architecture`, `Format`, `Lint`, `Check`, `Test`, `Build`. Значение по умолчанию
`All` выполняет каждый этап и останавливается на первой ошибке. `cargo lint`
и `cargo typecheck` — короткие host-native команды для локальной разработки.

## Hooks и CI

После clone установить `git config --local core.hooksPath .githooks`.
Git должен видеть executable `.githooks/pre-push`; hook вызывает `pwsh` и
`scripts/verify.ps1`. Ошибка gate блокирует push. Не использовать `--no-verify`.
Проверить `git config --get core.hooksPath` и выполнить gate на новом checkout.

GitHub Actions запускает тот же PowerShell gate на Windows для push в `main`
и pull requests. Успешная release-сборка публикуется как CI artifact без
автоматического релиза. Credentials, серверные профили и приватные ключи
не должны попадать в Git или артефакты.

Тесты application проверяют сохранение состояния backend и различение
недоступности backend от отключённого туннеля. GUI дополнительно проверяется
ручным запуском на реальном desktop. Сборка не заменяет runtime-проверку.
