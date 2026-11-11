# Платформенный контракт

| Платформа | Обязательство текущего этапа |
| --- | --- |
| Windows x64, `x86_64-pc-windows-msvc` | Локальные проверки и сборка native GUI + headless |
| Linux | Целевая будущая платформа; отдельные build/runtime gates ещё не выполнены |
| macOS | Целевая будущая платформа; нужен Apple build/runtime и packaging contract |
| Android | Будущая native оболочка, системный VPN API, permissions и lifecycle |
| iOS | Будущая native оболочка, Apple signing и Network Extension |
| OpenWrt / роутеры | Будущий headless backend без GUI; отдельно target/ABI/libc и ресурсный профиль |
| Другие устройства | Поддержка определяется отдельным целевым контрактом |

Домен и application не привязаны к Windows. Общий backend используется двумя
entrypoint; headless dependency graph не включает GUI/window/webview библиотеки.
Текущий `vpn-headless status` — завершающийся запрос, не daemon, IPC-сервис или
подтверждённая сборка для OpenWrt. Политика проверок — только локальная, без CI.

Системные API и особенности VPN-туннеля принадлежат внешним адаптерам с
целевыми зависимостями. Не устанавливать Windows target глобальным Cargo default.
Текущий toolchain закреплён для Windows MSVC; при введении других host/platform
profiles требуется явная настройка toolchain и отдельная проверка каждого.

Переносимость исходников не доказывает GUI, VPN API, фоновые службы, ABI,
маршрутизацию, DNS или packaging на другой ОС. Текущая Windows GUI/headless
сборка не доказывает Windows VPN: [core platforms](https://github.com/ArthurKoba/shoes/blob/master/docs/platforms.md)
владеют статусом модифицированного ядра. Инсталлятор и подпись релиза пока
не настроены. Мобильное выполнение внешних модулей зависит также от store policy;
целевые trust-критерии находятся в [product requirements](product-requirements.md).
