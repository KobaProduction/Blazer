# Установка и обновление Windows

Основной формат поставки — единый NSIS Setup для Windows x64. Он содержит
Blazer, закреплённый `blazecore.exe`, Wintun и нужные лицензии. Мастер RU/EN
использует фиксированный machine-wide каталог `C:\Program Files\Blazer`.
Данные профилей, подписок и настроек остаются в пользовательском каталоге ядра
клиента и не заменяются файлами установщика.

Установщик сначала работает от имени обычного пользователя. Только после
подтверждения установки он запрашивает UAC, сохраняет исходный SID оператора
и проверяет его через живой bootstrap. Отмена мастера или UAC не закрывает
приложение. Перед заменой файлов installer закрывает только процессы с
подтверждёнными путём продукта и владельцем, с ограниченным ожиданием;
нет широкого `taskkill` по имени. Обновление службы и файлов относится к одной
операции с резервными файлами и восстановлением при ошибке.

В текущей рабочей системе отдельно установлена `BlazeCoreTun` из
`Program Files\BlazeCore`. Новый Setup не считает такую службу своей:
он отклоняет конфликт и не останавливает, не удаляет и не переназначает её.
Прототип не содержит автоматического принятия чужой установки или migration.
Дальнейшая native-приёмка на этой машине требует отдельного решения владельца
о прежней standalone-службе; сейчас её состояние не изменяется.

Управляемая установка определяется protected HKLM64
`Software\KobaProduction\Blazer` и `blazer-installation.json`. Они содержат
совпадающие product identity, version, channel, sequence и commit; каталог
должен соответствовать фиксированному Program Files. Updater не превращает
старый portable ZIP в установленный продукт: он показывает путь к Setup.

## Релизы

Маркетинговая версия — `X.Y`; стабильный релиз задаётся Git-тегом `vX.Y`.
Preview из `main` хранится в единственном изменяемом release `preview`.
Sequence — количество first-parent коммитов, identity — полный SHA коммита.
Новый preview того же `X.Y` предлагается при другом коммите и большем sequence.
Stable никогда не получает preview; preview может перейти на stable с тем же `X.Y`.
Stable создаётся только из чистого checkout тега `vX.Y`. Обычный `-Installer`
создаёт preview из чистого checkout. Незакоммиченные исходники проверяются через
`-DevelopmentInstaller`: отдельный development executable с новым клиентом,
который не устанавливается, не запрашивает UAC и не публикуется. Его базовый Git
коммит не выдаётся за immutable identity изменённых байтов.

Файлы: `Blazer-X.Y-windows-x86_64-setup.exe` либо
`Blazer-preview-COMMIT12-SEQUENCE-windows-x86_64-setup.exe` и парный `-update.json`.
JSON schema 1 содержит version, channel, sequence, commit и setup
`{name,size,sha256}`. Updater сверяет JSON и Setup с digest/размером GitHub API,
ограничивает размер и точный URL репозитория, повторно проверяет скачанный файл
и удерживает его от замены до завершения мастера. Authenticode-подпись не заявлена.

Запуск установки из приложения — обычный процесс Setup с `/UPDATE`.
Окно остаётся открытым до подтверждения мастера; elevated installer владеет
остановкой клиента и службы, заменой и восстановлением. После ошибки в ещё
открытом приложении виден код/причина. Остатки собственных staged setup очищаются
при следующем старте ограниченным проходом: только отмеченные каталоги,
без reparse points и посторонних файлов; используемый executable не удаляется.

ZIP остаётся внутренним источником закреплённого ядра для следующего release-build.
Приложение не устанавливает ZIP и не содержит старого протокола self-copy updater.

## Проверено и что ещё требуется

23 ноября 2026 года выполнен один полный `scripts/verify.ps1`: architecture,
format, Clippy с cognitive complexity 15, check, 254 tests passed / 0 failed /
16 ignored и release-build. Фокусные тесты проверяют каналы/identity, подмену
метаданных и Setup, SHA-256/размер, read/delete lock и сохранность чужих/активных
staging-каталогов. Проверка feed не означает установку службы.

`scripts/installer/test-installer.ps1` прошёл 8 групп под Windows PowerShell 5.1
с fake SCM/registry/ACL/shortcut adapters и реальными файлами в изолированном TEMP:
graceful close и relaunch outcome, backup/rollback, сохранение foreign files,
atomic JSON, crash-cleanup tombstone, failure injection, версия и прежние
Running/Stopped/Disabled состояния. Это не native-мутации рабочей системы.

С новым клиентом собран неустанавливаемый development artifact
`dist/windows/Blazer-development-e9f46187529b-7-windows-x86_64-setup.exe`:
46 185 201 байт, SHA-256
`386C57083892DCC9B18D32B4E73283F18A1E29BBFF06AB6A232717DAC2693132`.
Проверены ProductVersion `0.1`, FileVersion `0.1.0.0`, извлечение 20 файлов с
проверкой всех payload digests, нового клиента и закреплённых core/Wintun.
Development `/UPDATE` отклонён до UAC/SCM. Этот артефакт не опубликован и не
является immutable release незакоммиченных исходников.

Полный native сценарий install → update →
rollback → uninstall с реальным SCM/Wintun пока не принят: изолированная Windows VM
в текущем окружении отсутствует. Обычная рабочая система для этой проверки
не изменяется. На Android и iOS сохраняются собственные правила магазинов.
