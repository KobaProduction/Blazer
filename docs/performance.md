# Измерение ресурсов

Память, CPU, энергия и лишний сетевой обмен — самостоятельные критерии
[продукта](product-requirements.md#ресурсы-и-доказательство-эффективности).
Этот документ владеет методикой baseline. Он не вводит численные performance
budgets до измерений и не объявляет Rust или выбранный GUI экономичнее Happ.

## Локальный idle probe

`scripts/measure-idle.ps1` измеряет только явно выбранный наш Windows EXE.
Он не собирает приложение и не запускает тесты. Обязательный expected SHA256
связывает запуск с проверенным артефактом. Скрипт делает отдельную копию в
указанный output directory, сверяет hash и запускает именно её, чтобы не держать
оригинальный build output во время relink.

Запуск запрашивает `WindowStyle Hidden`; это workload **hidden-idle-requested**,
а не проверка видимого GUI или его UX. Реальная видимость окна инструментом
не подтверждается. Никаких нажатий подключения или изменений VPN/routes/DNS
скрипт не делает. Он наблюдает только собственный PID и завершает только
процесс с совпадающими copy path и start time. При недоступных метриках выдаёт
UNVERIFIED, при ошибке прекращается с ненулевым кодом.

После отдельного warmup измеряются wall time, CPU time delta, CPU как доля
одного логического ядра, Private Bytes и Working Set. Peak в отчёте означает
максимум среди samples, а не гарантированный process lifetime peak.
CPU-проценты могут превышать 100 при многопоточной работе.
Это **не** измерение батареи, wakeups, allocations, throughput или энергии.

Пример одного измерения; SHA256 нужно брать от конкретной проверенной сборки:

```powershell
cd C:\Users\Koba\RustroverProjects\vpn-client
$binaryHash = (Get-FileHash -LiteralPath 'C:\path\to\vpn-client.exe' -Algorithm SHA256).Hash
pwsh -NoProfile -File scripts/measure-idle.ps1 -ExecutablePath 'C:\path\to\vpn-client.exe' -ExpectedSha256 $binaryHash -OutputDirectory 'C:\path\to\work\performance' -SourceRelation 'exact build/source relation'
```

Duration/warmup/sample interval — параметры workload, а не нормы приемлемого
потребления. Измерение не сравнивает Happ, не читает другие процессы и не
собирает трафик. Raw report и проверенная копия остаются локально в output
directory; их нельзя автоматически превращать в публичную диагностику.

## Воспроизводимый продуктовый baseline

Для каждого будущего platform profile фиксируются target/ABI, OS и hardware,
режим питания, source/build identity приложения и ядра, build flags, dataset,
workload и способ измерения. GUI visible, GUI hidden и headless backend
измеряются раздельно; потом — весь наш process tree. Сравнение старого и нового
артефактов требует одинаковых условий и повторяемых прогонов.

Помимо idle нужны контролируемые import/refresh, connect/disconnect/cancel,
steady data path, network loss/reconnect и отказ telemetry exporter. Эти
сценарии выполняются только после появления соответствующего проверенного
backend и отдельного разрешённого стенда. Mobile background и OpenWrt требуют
реального устройства, а не переноса desktop-результата.

До заявлений об энергоэффективности нужны реальные platform power measurements
и одинаковый полезный workload. CPU time — полезный вспомогательный сигнал,
но не замена энергии. Число повторов, длительность, статистика и численные
бюджеты утверждаются после пилотного baseline. Результат одного короткого
idle probe — наблюдение о конкретном binary, не performance verdict продукта.

## Полученные результаты

### Первое короткое наблюдение GUI-scaffold

Запуск 11 ноября 2026 года, 18:02:30–18:02:41 UTC (21:02 по Москве).
Run ID: `hidden-idle-20261111T180228289Z-510231f5d88440fe8919102a3205524b`.
Полный локальный `measurement.json` сохранён в рабочей папке измерений.
Измерение выполнено скриптом SHA256
`213BE45E8E0E0B1C399A1B2B67FF3F26661ED4AC6AD9D0C1FB58950743C0F15B`.
После него ожидание завершения собственного процесса ограничено 5 секундами;
метрики не переснимались ради этой правки cleanup.

Binary SHA256: `3326591CB922BFDDB37A1164707A0D0837F5ABA02016001F8FF4FE52F7B3A462`.
Это первая локальная release-сборка GUI-scaffold до initial commit `c43dbfe`;
точный source tree сборки не аттестован. Это **не** baseline новых backend,
headless или OTel-изменений. SHA256 отдельной запущенной копии совпал.

| Поле отчёта | Наблюдение |
| --- | --- |
| Outcome | `PROCESS_METRICS_OBSERVED` |
| Workload | `hidden-idle-requested`; WindowStyle Hidden запрошен, видимость не подтверждена |
| Warmup / запрошенная длительность / sample interval | 2 s / 10 s / 1 s |
| Измеренный loop wall time | 10 075.524 ms |
| CPU delta / доля одного логического ядра | 656.25 ms / 6.513% |
| Private Bytes: последняя / максимум samples | 62 377 984 / 114 188 288 bytes |
| Working Set: последняя / максимум samples | 84 783 104 / 103 845 888 bytes |
| Samples | 11 |
| Cleanup | Собственный PID 93956 завершён; identity checks и `CleanupVerified=true` |
| Энергия | Не измерялась |

Счётчик CPU вырос в первом интервале после warmup, затем не менялся на
разрешении инструмента. Короткий прогрев мог захватить отложенную инициализацию
GUI/driver. Это одиночное наблюдение под текущей нагрузкой хоста, без закреплённых
hardware/power условий, измеренного observer overhead и статистики повторов.
Wall time loop и timestamps samples включают несколько разные границы сбора;
цифры не следует трактовать как точный steady-idle regression threshold.

Результат подтверждает доступность измерения собственного процесса, но не
видимый GUI, качество VPN, эффективность батареи или превосходство над Happ.
Численные продуктовые бюджеты по этому одному запуску не установлены.
