# Blazer — карта для AI-агентов

Использовать «Blazer» в интерфейсе и `KobaProduction/Blazer` для репозитория.
`vpn-client` — техническое имя пакета и исполняемого файла клиента.
Это один Rust-клиент с общими сценариями приложения и несколькими способами
запуска. Текущие цели — Windows и Android.

## Обязательно перед работой

1. Прочитать этот файл и [README](README.md), проверить ветку и рабочее дерево. Для продуктовых сценариев читать [требования](docs/product-requirements.md) и [матрицу Happ](docs/reference-behavior.md).
2. Для любых изменений исходников или архитектуры полностью прочитать
   [Software Engineering](https://github.com/ArthurKoba/ai-agent-workflow/blob/main/skills/software-engineering/README.md),
   [Clean Architecture / DDD](https://github.com/ArthurKoba/ai-agent-workflow/blob/main/skills/software-engineering/clean-architecture-ddd.md)
   из [AI Agent Workflow](https://github.com/ArthurKoba/ai-agent-workflow)
   и [архитектурный контракт проекта](docs/architecture.md).
3. Читать целиком выбранные ниже источники по текущей задаче. Если обязательный
   источник недоступен, остановить зависящую от него работу и сообщить причину.
   Не копировать универсальные инструкции в репозиторий.

## Маршруты по задаче

| Задача | Обязательный источник |
| --- | --- |
| Toolchain, зависимости, линтер, hooks, локальные проверки | [Project Bootstrap](https://github.com/ArthurKoba/ai-agent-workflow/blob/main/skills/project-bootstrap.md), [разработка](docs/development.md) |
| Git, коммиты и публикация | [Git Workflow](https://github.com/ArthurKoba/ai-agent-workflow/blob/main/skills/git-workflow.md) и выбранные им модули |
| Документация и маршрутизация контекста | [Context Engineering](https://github.com/ArthurKoba/ai-agent-workflow/blob/main/skills/context-engineering.md) и выбранные им модули |
| Команды shell / PowerShell | [Terminal Operations](https://github.com/ArthurKoba/ai-agent-workflow/blob/main/skills/terminal-operations.md) |
| Новая платформа или системный VPN-адаптер | [Платформенный контракт](docs/platforms.md), архитектурный контракт |

## Владельцы информации

- Код и зависимости: `src/{domain,application,infrastructure,backend}`,
  `src/presentation/` и Cargo-манифесты.
- Границы слоёв: [docs/architecture.md](docs/architecture.md); core contracts принадлежат [ArthurKoba/BlazeCore](https://github.com/ArthurKoba/BlazeCore).
- Проверки: [docs/development.md](docs/development.md), `scripts/verify.ps1`.
- Задачи, решения и незавершённая работа: GitHub Issues этого репозитория.
- Credentials и локальная инфраструктура: вне отслеживаемых файлов.

Проверки сложности и границ слоёв обязательны. Когнитивная сложность функции
не должна превышать 15; не подавлять правило и не повышать порог ради прохождения.
Проверять Windows и Android отдельно. Результаты сборки и сетевого теста одной
платформы не переносятся на другую.

## Локальный цикл по решению владельца

Проверки выполняются только локально. Не запускать CI, не ожидать его результата
и не восстанавливать автоматический workflow. Это явное исключение владельца
из общего CI-default навыка Project Bootstrap; локальные format/lint/check/test/build
и обязательный pre-push остаются действующими. Во время разработки запускать
только проверки затронутого поведения; полный финальный набор — один раз после
стабилизации перед commit или отправкой изменений.

Для обычных локальных сборок переиспользовать стандартные пути упаковки.
Запускать клиент из `dist/windows/client`, демо из `dist/windows/demo`.
Отдельный каталог указывать только для артефакта, который нужно сохранить.

Для Android на Windows сначала переиспользовать совместимую Java и один
существующий каталог SDK с правом записи. Хранить SDK, Java и данные AVD
вне рабочих каталогов Codex. Использовать постоянные пользовательские
переменные `ANDROID_HOME`, `JAVA_HOME` и `Path`.
Точные пакеты и настройка описаны в [разделе Android](README.md#сборка-для-android).
