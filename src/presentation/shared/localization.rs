//! Central RU/EN catalog. Protocol and user-supplied data are never translated.

use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    Russian,
    English,
}

impl Locale {
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "ru" | "ru-RU" => Self::Russian,
            "en" | "en-US" | "en-GB" => Self::English,
            _ => Self::English,
        }
    }
}

macro_rules! messages {
    ($( $key:ident => ($id:literal, $ru:literal, $en:literal) ),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Key { $( $key ),+ }

        impl Key {
            pub const ALL: &'static [Self] = &[ $( Self::$key ),+ ];
            pub fn id(self) -> &'static str {
                match self { $( Self::$key => $id ),+ }
            }
        }

        pub fn text(locale: Locale, key: Key) -> &'static str {
            match (locale, key) {
                $( (Locale::Russian, Key::$key) => $ru,
                   (Locale::English, Key::$key) => $en ),+
            }
        }

        pub fn lookup<'a>(locale: Locale, id: &'a str) -> Cow<'a, str> {
            match id {
                $( $id => Cow::Borrowed(text(locale, Key::$key)), )+
                _ => Cow::Borrowed(id),
            }
        }
    }
}

messages! {
    InterfaceSettings => ("desktop.settings.interface", "Настройки интерфейса", "Interface settings"),
    Theme => ("desktop.settings.theme", "Тема", "Theme"),
    ThemeSystem => ("desktop.settings.theme.system", "Системная", "System"),
    ThemeDark => ("desktop.settings.theme.dark", "Тёмная", "Dark"),
    ThemeLight => ("desktop.settings.theme.light", "Светлая", "Light"),
    TunnelSettings => ("desktop.settings.tunnel", "Настройки туннеля", "Tunnel settings"),
    RoutingRules => ("desktop.settings.routing", "Правила маршрутизации", "Routing rules"),
    PerAppProxy => ("desktop.settings.per_app", "Настройки прокси для приложений", "Per-app proxy settings"),
    PerAppTunRequired => ("desktop.settings.per_app.requires_tun", "Требуется включить режим TUN", "Enable TUN mode to use per-app proxy settings"),
    Fragmentation => ("desktop.settings.tunnel.fragmentation", "Включить фрагментацию", "Enable fragmentation"),
    Multiplex => ("desktop.settings.tunnel.multiplex", "Включить мультиплексор", "Enable multiplexing"),
    PreferredIp => ("desktop.settings.tunnel.preferred_ip", "Предпочитаемый тип IP", "Preferred IP type"),
    Automatic => ("desktop.settings.automatic", "Авто", "Automatic"),
    Inbounds => ("desktop.settings.inbounds", "Входящие подключения", "Inbound connections"),
    AdvancedSettings => ("desktop.settings.advanced", "Дополнительные настройки", "Advanced settings"),
    AllowLan => ("desktop.settings.tunnel.lan", "Разрешить подключения из LAN", "Allow LAN connections"),
    OtherSettings => ("desktop.settings.other", "Другие", "Other"),
    ResetSettings => ("desktop.settings.reset", "Сброс", "Reset"),
    ResetSettingsConfirm => ("desktop.settings.reset.confirm", "Сбросить настройки интерфейса и локальные черновики?", "Reset interface settings and local drafts?"),
    Faq => ("desktop.settings.faq", "Часто задаваемые вопросы", "Frequently asked questions"),
    UrlSchemes => ("desktop.settings.url_schemes", "URL-схемы", "URL schemes"),
    SettingEnabled => ("desktop.settings.state.enabled", "Включено", "Enabled"),
    SettingDisabled => ("desktop.settings.state.disabled", "Отключено", "Disabled"),
    SettingsPreviewOnly => ("desktop.settings.preview_only", "Черновик текущего сеанса. Для применения этих параметров требуется поддержка ядра.", "Session draft. Applying these options requires backend support."),
    DesktopScale => ("desktop.settings.interface.scale", "Масштаб интерфейса", "Interface scale"),
    DesktopScaleSystem => ("desktop.settings.interface.scale.system", "Системное ({scale})", "System ({scale})"),
    CidrInputHint => ("desktop.settings.cidr_input_hint", "Например, 10.0.0.0/8", "For example, 10.0.0.0/8"),
    CidrKeysHelp => ("desktop.settings.cidr_keys_help", "Enter — добавить, Esc — отменить.", "Enter to add, Esc to cancel."),
    GraphicsBackend => ("desktop.settings.interface.graphics_backend", "Графический бэкенд", "Graphics backend"),
    GraphicsBackendHelp => ("desktop.settings.interface.graphics_backend_help", "Текущая сборка использует OpenGL.", "This build uses OpenGL."),
    JsonDns => ("desktop.settings.advanced.json_dns", "Использовать DNS из JSON", "Use DNS from JSON"),
    ResolveServer => ("desktop.settings.advanced.resolve", "Резолвить адрес сервера", "Resolve the server address"),
    Sniffing => ("desktop.settings.advanced.sniffing", "Анализ пакетов", "Packet sniffing"),
    Autostart => ("desktop.settings.advanced.autostart", "Автозапуск", "Start with Windows"),
    BypassLocalProxy => ("desktop.settings.advanced.bypass_local", "Обходить прокси для локальных адресов", "Bypass the proxy for local addresses"),
    EnableTun => ("desktop.settings.advanced.enable_tun", "Включить режим TUN", "Enable TUN mode"),
    ExcludedRoutes => ("desktop.settings.advanced.excluded_routes", "Исключённые маршруты", "Excluded routes"),
    CidrList => ("desktop.settings.advanced.excluded_routes.cidr_list", "Список CIDR", "CIDR list"),
    AddCidr => ("desktop.settings.advanced.excluded_routes.add_cidr", "Добавить CIDR…", "Add CIDR…"),
    CidrHelp => ("desktop.settings.advanced.excluded_routes.help", "Трафик к этим адресам направляется напрямую. Подписка может дополнить или заменить этот список.", "Traffic to these addresses goes directly. A subscription can add to or replace this list."),
    UpdateNotification => ("desktop.settings.subscriptions.notification", "Показывать ошибки автообновления", "Show automatic refresh errors"),
    RequestTimeout => ("desktop.settings.subscriptions.timeout", "Таймаут запроса", "Request timeout"),
    SubscriptionScheduleDraft => ("desktop.settings.subscriptions.schedule_draft", "Эти настройки доступны как черновик текущего сеанса. Фактическое расписание определяется правилами поставщика подписки.", "These controls are a session draft. The subscription provider’s rules determine the live update schedule."),
    PreventDuplicates => ("desktop.settings.subscriptions.prevent_duplicates", "Запретить дубликаты", "Prevent duplicates"),
    CollapseSubscriptions => ("desktop.settings.subscriptions.collapse", "Сворачивать подписки", "Collapse subscriptions"),
    DisableSubscriptionFilter => ("desktop.settings.subscriptions.disable_filter", "Отключить фильтрацию", "Disable filtering"),
    SendHwid => ("desktop.settings.subscriptions.send_hwid", "Передавать HWID", "Send HWID"),
    SubscriptionUserAgent => ("desktop.settings.subscriptions.user_agent", "User-Agent подписок", "Subscription User-Agent"),
    SubscriptionUserAgentHelp => ("desktop.settings.subscriptions.user_agent.help", "Оставьте поле пустым, чтобы использовать стандартный User-Agent. Изменение применяется при следующей загрузке подписки.", "Leave this field blank to use the default User-Agent. The change takes effect the next time a subscription is fetched."),
    SubscriptionUserAgentInvalid => ("desktop.settings.subscriptions.user_agent.invalid", "Введите до 512 печатных ASCII-символов без пробелов по краям.", "Enter up to 512 printable ASCII characters with no leading or trailing spaces."),
    ServerSort => ("desktop.settings.subscriptions.sort", "Сортировка серверов", "Server sorting"),
    SortOriginal => ("desktop.settings.subscriptions.sort.original", "Без сортировки", "Original order"),
    SortPing => ("desktop.settings.subscriptions.sort.ping", "По пингу", "By ping"),
    SortAlphabetical => ("desktop.settings.subscriptions.sort.alphabetical", "По алфавиту", "Alphabetical"),
    HoursValue => ("desktop.settings.hours_value", "{value} ч", "{value} h"),
    SecondsValue => ("desktop.settings.seconds_value", "{value} с", "{value} s"),
    PingMethodHelp => ("desktop.settings.ping.method_help", "Авто: TCP для TCP-серверов, HTTP-запрос через профиль для UDP и QUIC. При активном TUN проверяется действующий туннель.", "Automatic: TCP for TCP servers, an HTTP request through the profile for UDP and QUIC. While TUN is active, the current tunnel is measured."),
    PingTcp => ("desktop.settings.ping.tcp", "TCP", "TCP"),
    PingTcpHelp => ("settings.ping.tcp_help", "TCP: время установления соединения с адресом сервера напрямую. Проверка данных внутри открытого соединения требует прикладного протокола.", "TCP: measures a direct connection's establishment time to the server. Testing data within an open connection requires an application protocol."),
    PingIcmp => ("settings.ping.icmp", "ICMP", "ICMP"),
    PingIcmpHelp => ("settings.ping.icmp_help", "Время ответа ICMP Echo от адреса сервера. Сервер или сеть могут блокировать ICMP.", "Measures an ICMP Echo reply from the server address. The server or network may block ICMP."),
    PingIcmpUnavailable => ("settings.ping.icmp_unavailable", "ICMP недоступен на этой платформе.", "ICMP is unavailable on this platform."),
    PingProxy => ("settings.ping.proxy", "Через прокси", "Through proxy"),
    PingProxyHelp => ("settings.ping.proxy_help", "HTTP-запрос через выбранный профиль. При активном TUN используется действующий туннель.", "Sends an HTTP request through the selected profile. While TUN is active, the current tunnel is used."),
    PingHttpMethod => ("settings.ping.http_method", "HTTP-метод", "HTTP method"),
    PingGet => ("settings.ping.get", "GET", "GET"),
    PingHead => ("settings.ping.head", "HEAD", "HEAD"),
    PingUrl => ("settings.ping.url", "URL проверки", "Probe URL"),
    PingTimeout => ("settings.ping.timeout", "Таймаут измерения (мс)", "Measurement timeout (ms)"),
    PingAttempts => ("settings.ping.attempts", "Измерений без прогрева", "Measurements excluding warmup"),
    PingWarmup => ("settings.ping.warmup", "Прогрев перед измерением", "Warm up before measuring"),
    PingWarmupHelp => ("settings.ping.warmup_help", "Предварительная проверка не входит в число измерений и итоговый результат. HTTP-запросы повторно используют прогретое соединение.", "The preliminary check is excluded from the measurement count and result. HTTP requests reuse the warmed-up connection."),
    PingWarmupMandatory => ("settings.ping.warmup_mandatory", "Для HTTPS прогрев обязателен: начальное TCP/TLS-соединение создаётся до измерений. Выберите ресурс с поддержкой постоянных соединений.", "HTTPS warmup is mandatory: the initial TCP/TLS connection is established before measurements. Choose a resource that supports persistent connections."),
    PingIcmpWarmupHelp => ("settings.ping.icmp_warmup", "ICMP измеряет время ответа Echo; прогрев HTTP/TCP-соединения к нему не применяется.", "ICMP measures the Echo reply time; HTTP/TCP connection warmup does not apply."),
    ProbeConnectionNotReusable => ("error.probe_connection_not_reusable", "Ресурс проверки закрыл прогретое соединение. Укажите URL с поддержкой постоянных HTTP-соединений.", "The probe resource closed the warmed-up connection. Choose a URL that supports persistent HTTP connections."),
    PingStrategy => ("settings.ping.strategy", "Итоговый результат", "Result aggregation"),
    PingLast => ("settings.ping.last", "Последний", "Last"),
    PingMinimum => ("settings.ping.minimum", "Минимальный", "Minimum"),
    PingAverage => ("settings.ping.average", "Среднее", "Average"),
    PingMedian => ("settings.ping.median", "Медиана", "Median"),
    PingSettingsSaved => ("settings.ping.saved", "Настройки проверки сохранены.", "Probe settings saved."),
    PingTime => ("desktop.settings.ping.display.time", "Время", "Time"),
    PingIcon => ("desktop.settings.ping.display.icon", "Иконка", "Icon"),
    InboundListener => ("desktop.settings.inbounds.listener", "Локальный прокси · SOCKS5 и HTTP CONNECT", "Local proxy · SOCKS5 and HTTP CONNECT"),
    InboundPort => ("desktop.settings.inbounds.port", "Порт для следующего подключения", "Port for the next connection"),
    InboundPortHelp => ("desktop.settings.inbounds.port_help", "Порт 0 означает автоматический выбор. Изменение вступит в силу при следующем подключении в режиме «Локальный прокси».", "Use port 0 to select a free port automatically. Changes take effect the next time you connect in Local Proxy mode."),
    InboundPortInvalid => ("desktop.settings.inbounds.port_invalid", "Введите число от 0 до 65535.", "Enter a number between 0 and 65535."),
    InboundAuthUnavailable => ("desktop.settings.inbounds.auth_unavailable", "Слушатель доступен только на этом устройстве. Настройка авторизации недоступна.", "The listener is available only on this device. Authentication settings are unavailable."),
    InboundAuthMode => ("desktop.settings.inbounds.auth_mode", "Режим авторизации", "Authentication mode"),
    InboundNoAuth => ("desktop.settings.inbounds.no_auth", "Без авторизации", "No authentication"),
    InboundPassword => ("desktop.settings.inbounds.password", "Логин и пароль", "Username and password"),
    SchemeImport => ("desktop.settings.url_schemes.import", "Импорт в Blazer", "Import into Blazer"),
    SchemeImportHelp => ("desktop.settings.url_schemes.import_help", "Вставьте ссылку в форму импорта или передайте её при запуске Blazer. Поддерживаются URL подписок и конфигураций серверов.", "Paste the link into the import form or pass it when launching Blazer. Subscription URLs and server configuration URLs are supported."),
    RoutingProfile => ("desktop.settings.routing.profile", "Профиль маршрутизации", "Routing profile"),
    RoutingProfileMissing => ("routing.profile_missing", "Профиль маршрутизации не найден.", "The routing profile was not found."),
    RoutingProfileAssigned => ("routing.profile_assigned", "Сначала снимите назначение профиля маршрутизации.", "Unassign the routing profile first."),
    RoutingActiveProfile => ("desktop.routing.active_profile", "Применённый профиль", "Active profile"),
    RoutingAddRule => ("desktop.routing.add_rule", "Добавить правило", "Add rule"),
    RoutingBlock => ("desktop.routing.block", "Блокировать", "Block"),
    RoutingBuiltInFallback => ("desktop.routing.built_in_fallback", "Стандартная маршрутизация", "Default routing"),
    RoutingCidr => ("desktop.routing.cidr", "Подсеть CIDR", "CIDR range"),
    RoutingCopyProfile => ("desktop.routing.copy_profile", "Копировать профиль", "Copy profile"),
    RoutingCopyTitle => ("desktop.routing.copy_title", "{title} (копия)", "{title} (copy)"),
    RoutingDirect => ("desktop.routing.direct", "Напрямую", "Direct"),
    RoutingDnsEndpoint => ("desktop.routing.dns_endpoint", "Адрес и порт", "Address and port"),
    RoutingDnsEndpointInvalid => ("desktop.routing.dns_endpoint_invalid", "Введите IP-адрес и порт DNS-сервера.", "Enter the DNS server’s IP address and port."),
    RoutingDnsHttps => ("desktop.routing.dns_https", "DNS через HTTPS", "DNS over HTTPS"),
    RoutingDnsPath => ("desktop.routing.dns_path", "Путь HTTPS", "HTTPS path"),
    RoutingDnsSettings => ("desktop.routing.dns_settings", "Tunnel DNS", "Tunnel DNS"),
    RoutingDnsSystem => ("desktop.routing.dns_system", "Системный DNS", "System DNS"),
    RoutingDnsTcp => ("desktop.routing.dns_tcp", "DNS через TCP", "DNS over TCP"),
    RoutingDnsTls => ("desktop.routing.dns_tls", "DNS через TLS", "DNS over TLS"),
    RoutingDomainSuffix => ("desktop.routing.domain_suffix", "Суффикс домена", "Domain suffix"),
    RoutingExactDomain => ("desktop.routing.exact_domain", "Точный домен", "Exact domain"),
    RoutingFirstMatch => ("desktop.routing.first_match", "Применяется первое подходящее правило.", "The first matching rule applies."),
    RoutingLocalProxyOnly => ("desktop.routing.local_proxy_only", "Профили маршрутизации применяются в режиме локального прокси.", "Routing profiles apply in local proxy mode."),
    RoutingLocalProxyAndTun => ("desktop.routing.local_proxy_and_tun", "Профили маршрутизации применяются в режимах локального прокси и TUN.", "Routing profiles apply in local proxy and TUN modes."),
    RoutingMoveDown => ("desktop.routing.move_down", "Ниже", "Move down"),
    RoutingMoveUp => ("desktop.routing.move_up", "Выше", "Move up"),
    RoutingPendingReconnect => ("desktop.routing.pending_reconnect", "Сохранённые правила вступят в силу после переподключения.", "Saved rules will take effect after reconnecting."),
    RoutingConfirmReconnect => ("desktop.routing.confirm_reconnect", "Применить изменения маршрутизации?", "Apply routing changes?"),
    RoutingApplyNow => ("desktop.routing.apply_now", "Применить сейчас", "Apply now"),
    RoutingApplyNextConnection => ("desktop.routing.apply_next_connection", "При следующем подключении", "On next connection"),
    RoutingProxy => ("desktop.routing.proxy", "Через прокси", "Proxy"),
    RoutingRemoveTitle => ("desktop.routing.remove_title", "Удалить профиль маршрутизации?", "Remove the routing profile?"),
    RoutingRevisionLabel => ("desktop.routing.revision_label", "{title} · ред. {revision}", "{title} · rev. {revision}"),
    RoutingRuleAction => ("desktop.routing.rule_action", "Действие", "Action"),
    RoutingRuleTarget => ("desktop.routing.rule_target", "Тип адресата", "Target type"),
    RoutingRuleValue => ("desktop.routing.rule_value", "Адресат", "Target"),
    RoutingSavedProfile => ("desktop.routing.saved_profile", "Сохранённый профиль", "Saved profile"),
    RoutingUseGlobal => ("desktop.routing.use_global", "Общие правила", "Global rules"),
    RoutingExplanation => ("desktop.settings.routing.help", "Правила направляют трафик через Proxy (прокси), Direct (напрямую) или Block (блокировка).", "Rules direct traffic through Proxy, send it directly, or block it."),
    RoutingDefaultProfile => ("desktop.settings.routing.default_profile", "Стандартный профиль", "Default profile"),
    RoutingAddProfile => ("desktop.settings.routing.add_profile", "Добавить профиль", "Add profile"),
    RoutingImportClipboard => ("desktop.settings.routing.import_clipboard", "Импортировать из буфера обмена", "Import from clipboard"),
    RoutingImportQr => ("desktop.settings.routing.import_qr", "Импортировать QR-код", "Import QR code"),
    RoutingExportClipboard => ("desktop.settings.routing.export_clipboard", "Экспортировать в буфер обмена", "Export to clipboard"),
    RoutingEditGroup => ("desktop.settings.routing.destination_group", "Выберите группу", "Choose a group"),
    RoutingProfileTitle => ("desktop.settings.routing.title", "Заголовок", "Title"),
    RoutingCreate => ("desktop.settings.routing.create", "Создать", "Create"),
    RoutingNext => ("desktop.settings.routing.next", "Далее", "Next"),
    RoutingSource => ("desktop.settings.routing.source", "Источник", "Source"),
    RoutingLastUpdated => ("desktop.settings.routing.last_updated", "Последнее обновление", "Last updated"),
    RoutingDnsType => ("desktop.settings.routing.dns_type", "Тип", "Type"),
    RoutingDnsDomain => ("desktop.settings.routing.dns_domain", "Домен", "Domain"),
    RoutingDnsIp => ("desktop.settings.routing.dns_ip", "IP", "IP"),
    RoutingEnabled => ("desktop.settings.routing.enable", "Включить маршрутизацию", "Enable routing"),
    RoutingIgnoreImport => ("desktop.settings.routing.ignore_import", "Игнорировать импорт роутинга", "Ignore routing imports"),
    RoutingRuleSelection => ("desktop.settings.routing.rule_selection", "Выбор правила", "Rule selection"),
    RoutingCopyOther => ("desktop.settings.routing.copy_other", "Копировать из другой подписки", "Copy from another subscription"),
    RoutingManualGroup => ("desktop.settings.routing.manual_group", "Мои профили", "My profiles"),
    RoutingDomainStrategy => ("desktop.settings.routing.domain_strategy", "Стратегия доменов", "Domain strategy"),
    RoutingDomains => ("desktop.settings.routing.domains", "Домены", "Domains"),
    RoutingIps => ("desktop.settings.routing.ips", "IP-адреса", "IP addresses"),
    RoutingNoRules => ("desktop.settings.routing.no_rules", "В этой группе пока нет профилей.", "This group has no profiles yet."),
    RoutingGeneral => ("desktop.settings.routing.general", "Общие", "General"),
    RoutingBundled => ("desktop.settings.routing.bundled", "Встроенные", "Built-in"),
    PerAppSystem => ("desktop.settings.per_app.system", "Системные правила", "System routing"),
    PerAppDirect => ("desktop.settings.per_app.direct", "Напрямую для выбранных приложений", "Direct for selected apps"),
    PerAppOnly => ("desktop.settings.per_app.only", "Прокси только для выбранных приложений", "Proxy only for selected apps"),
    PerAppList => ("desktop.settings.per_app.list", "Список приложений", "Applications"),
    PerAppExecutable => ("desktop.settings.per_app.executable", "Путь к приложению", "Application path"),
    InboundLivePort => ("desktop.settings.inbounds.live_port", "Порт текущего подключения", "Port used by the active connection"),
    Servers => ("desktop.servers", "Серверы", "Servers"),
    ServerList => ("desktop.server_list", "Список серверов", "Server list"),
    ServerCatalogActions => ("desktop.server_catalog_actions", "Действия со списком серверов", "Server list actions"),
    AddUrl => ("desktop.add_url", "Добавить URL", "Add URL"),
    AddClipboard => ("import.clipboard", "Добавить из буфера", "Add from clipboard"),
    AddQr => ("import.qr", "Импорт QR-кода", "Import QR code"),
    ImportedServers => ("import.servers", "Импортированные серверы", "Imported servers"),
    ImportInvalidSubscription => ("import.subscription.invalid", "По этому адресу серверы не найдены. Скопируйте прямую ссылку на подписку.", "No servers were found at this address. Copy the direct subscription URL."),
    ImportInvalidUrl => ("import.subscription.invalid_url", "Проверьте прямую ссылку на подписку. Она должна начинаться с http:// или https://.", "Check the direct subscription URL. It should start with http:// or https://."),
    ImportInvalidJson => ("import.configuration.invalid_json", "Проверьте JSON или заново скопируйте экспортированный конфиг.", "Check the JSON or copy the exported configuration again."),
    ImportInvalidConfiguration => ("import.configuration.invalid", "Проверьте текст конфигурации или ссылку на сервер.", "Check the configuration text or server link."),
    ImportUnreadableConfiguration => ("import.configuration.unreadable_file", "Не удалось прочитать текст конфигурации.", "Could not read the configuration file."),
    ImportFileTooLarge => ("import.configuration.file_too_large", "Файл конфигурации больше 1 МБ.", "The configuration file exceeds 1 MB."),
    ImportUnsupportedConfiguration => ("import.configuration.unsupported", "Blazer пока не поддерживает этот формат или некоторые параметры конфигурации.", "Blazer does not support this format or some configuration settings yet."),
    ClipboardEmpty => ("import.clipboard.empty", "В буфере нет текста или QR-изображения. Скопируйте конфиг, подписку или снимок.", "The clipboard has no text or QR image. Copy a configuration, a subscription, or a screenshot."),
    ClipboardTextTooLarge => ("import.clipboard.too_large", "В буфере слишком большой текст. Скопируйте одну ссылку или один конфиг.", "The clipboard text is too large. Copy one link or configuration."),
    PasteQrImage => ("import.qr.paste", "Вставить снимок", "Paste screenshot"),
    QrOpenFile => ("import.qr.file", "Открыть изображение", "Open image file"),
    QrScreenshot => ("import.qr.screenshot", "Сделать снимок экрана", "Capture a screenshot"),
    QrScreenshotFailed => ("import.qr.capture_failed", "Снимок экрана недоступен. Попробуйте выделить область в «Ножницах» или открыть файл.", "Screen capture is unavailable. Try selecting a region in Snipping Tool or opening a file."),
    QrScreenshotHelp => ("import.qr.screenshot_help", "Нажмите Win+Shift+S и выделите QR-код. Вернитесь сюда и нажмите «Вставить снимок».", "Press Win+Shift+S and select the QR code. Return here and choose Paste screenshot."),
    QrScreenshotMobileHelp => ("import.qr.screenshot_mobile", "Выберите сохранённый снимок экрана из галереи.", "Choose a saved screenshot from the gallery."),
    QrNeedImage => ("import.qr.need_image", "Скопируйте снимок экрана с QR-кодом и вставьте его сюда.", "Copy a screenshot containing the QR code, then paste it here."),
    QrCamera => ("import.qr.camera", "Сканировать камерой", "Scan with camera"),
    QrCameraHelp => ("import.qr.camera_help", "Наведите камеру на QR-код.", "Point the camera at the QR code."),
    QrCameraUnavailable => ("import.qr.camera_unavailable", "Камера недоступна. Проверьте разрешение или откройте изображение.", "The camera is unavailable. Check the permission or open an image file."),
    QrUnreadableImage => ("import.qr.image", "Откройте изображение в формате PNG, JPEG или WebP.", "Open a PNG, JPEG or WebP image."),
    QrImageTooLarge => ("import.qr.size", "Обрежьте снимок вокруг QR-кода и попробуйте снова.", "Crop the image around the QR code and try again."),
    QrMissing => ("import.qr.missing", "QR-код не найден. Попробуйте более чёткий снимок.", "No QR code was found. Try a sharper image."),
    QrMultiple => ("import.qr.multiple", "На снимке несколько QR-кодов. Обрежьте его до одного кода.", "The image contains several QR codes. Crop it to one code."),
    ImportProblem => ("import.problem", "Импорт не завершён", "Import incomplete"),
    ImportReading => ("import.reading", "Читаем данные…", "Reading input…"),
    Retry => ("action.retry", "Повторить", "Try again"),
    PingAll => ("desktop.ping_all", "Пинг всех", "Ping all"),
    PingTest => ("desktop.ping_test", "Тест пинга", "Ping test"),
    PinSubscription => ("desktop.pin_subscription", "Закрепить", "Pin"),
    UnpinSubscription => ("desktop.unpin_subscription", "Открепить", "Unpin"),
    CopySubscriptionUrl => ("desktop.copy_subscription_url", "Копировать URL", "Copy URL"),
    ShowSubscriptionUrl => ("subscription.show_url", "Показать URL", "Show URL"),
    HideSubscriptionUrl => ("subscription.hide_url", "Скрыть URL", "Hide URL"),
    SearchServers => ("desktop.search_servers", "Введите текст для поиска", "Search servers"),
    NoMatchingServers => ("desktop.no_matching_servers", "Серверы не найдены", "No servers found"),
    NoAvailableServers => ("profile.none_available", "Нет доступных серверов", "No available servers"),
    UnnamedServer => ("profile.unnamed", "Сервер без названия", "Unnamed server"),
    ProfileUnsupported => ("profile.unavailable.unsupported", "Конфигурация не поддерживается", "Unsupported configuration"),
    ProfileUnsupportedXhttpUploadMethod => ("profile.unavailable.xhttp_upload_method", "Метод загрузки XHTTP не подходит для режима профиля. Обновите подписку, чтобы перепроверить профиль.", "The XHTTP upload method is incompatible with the profile mode. Refresh the subscription to check the profile again."),
    ProfileUnsupportedXhttpUploadMethodShort => ("profile.unavailable.xhttp_upload_method_short", "XHTTP · проверьте режим", "XHTTP · check mode"),
    ProfileInvalid => ("profile.unavailable.invalid", "Некорректная конфигурация", "Invalid configuration"),
    CatalogBlazeCoreTun => ("desktop.catalog_blazecore_tun", "BlazeCore TUN", "BlazeCore TUN"),
    CatalogUpdatedRecently => ("desktop.catalog_updated_recently", "Обновлено только что", "Updated just now"),
    CatalogUpdatedMinutes => ("desktop.catalog_updated_minutes", "Обновлено {count} мин назад", "Updated {count} min ago"),
    CatalogUpdatedHours => ("desktop.catalog_updated_hours", "Обновлено {count} ч назад", "Updated {count} h ago"),
    CatalogUpdatedDays => ("desktop.catalog_updated_days", "Обновлено {count} дн назад", "Updated {count} d ago"),
    CatalogAutoUpdateInterval => ("desktop.catalog_auto_update_interval", "Автообновление · {count} ч", "Auto-update · {count} h"),
    CatalogUsageDetails => ("desktop.catalog_usage_details", "Сведения подписки", "Subscription info"),
    CatalogTraffic => ("desktop.catalog_traffic", "{used} / {total}", "{used} / {total}"),
    CatalogTrafficUsed => ("desktop.catalog_traffic_used", "Использовано: {used}", "Used: {used}"),
    CatalogTrafficLimit => ("desktop.catalog_traffic_limit", "Лимит: {total}", "Limit: {total}"),
    CatalogTrafficUnlimited => ("desktop.catalog_traffic_unlimited", "Без лимита", "Unlimited"),
    CatalogUsageExpired => ("desktop.catalog_usage_expired", "Срок действия истёк", "Subscription expired"),
    CatalogUsageExpiresHours => ("desktop.catalog_usage_expires_hours", "До окончания: {count} ч", "Expires in {count} h"),
    CatalogUsageExpiresDays => ("desktop.catalog_usage_expires_days", "До окончания: {count} дн.", "Expires in {count} d"),
    CatalogProviderSupport => ("desktop.catalog_provider_support", "Поддержка", "Support"),
    CatalogProviderRenew => ("desktop.catalog_provider_renew", "Продлить", "Renew"),
    PingMilliseconds => ("desktop.ping_milliseconds", "{time} мс", "{time} ms"),
    PingTimedOut => ("desktop.ping_timed_out", "Истекло время ожидания", "Timed out"),
    PingUnreachable => ("desktop.ping_unreachable", "Сервер недоступен", "Server unreachable"),
    PingCheckUnreachable => ("desktop.ping_check_unreachable", "Контрольный адрес недоступен через профиль", "The check address is unreachable through this profile"),
    PingTcpTest => ("desktop.ping_tcp_test", "Проверить доступность", "Check reachability"),
    PingTcpResult => ("desktop.ping_tcp_result", "Проверка · {result}", "Check · {result}"),
    PingTunUnavailable => ("desktop.ping_tun_unavailable", "При активном TUN выберите «Авто» или «Через прокси» либо отключите туннель.", "While TUN is active, select Automatic or Through proxy, or disconnect the tunnel."),
    ExpandSidebar => ("desktop.expand_sidebar", "Развернуть меню", "Expand sidebar"),
    CollapseSidebar => ("desktop.collapse_sidebar", "Свернуть меню", "Collapse sidebar"),
    AddConfiguration => ("desktop.add_configuration", "Добавить конфигурацию", "Add configuration"),
    ConfigurationType => ("desktop.configuration_type", "Тип", "Type"),
    SubscriptionKind => ("desktop.subscription_kind", "Подписка", "Subscription"),
    ConfigurationKind => ("desktop.configuration_kind", "Конфигурация", "Configuration"),
    JsonKind => ("desktop.json_kind", "JSON", "JSON"),
    QrKind => ("desktop.qr_kind", "QR-код", "QR code"),
    QrDemoHelp => ("desktop.qr_demo_help", "Перетащите изображение или выберите образец для имитации распознавания QR.", "Drop an image or choose the sample to simulate QR recognition."),
    QrDemoChoose => ("desktop.qr_demo_choose", "Выбрать тестовое изображение", "Choose sample image"),
    QrDemoServer => ("desktop.qr_demo_server", "🇫🇮 Хельсинки · QR", "🇫🇮 Helsinki · QR"),
    QrDemoComplete => ("desktop.qr_demo_complete", "Пробный импорт выполнен. Серверы из QR можно добавить в деморежиме.", "Test import complete. QR profiles can be added in demo mode."),
    EditConfiguration => ("desktop.edit_configuration", "Редактировать конфигурацию", "Edit configuration"),
    ProfileSource => ("profile.source", "Исходная конфигурация сервера", "Original server configuration"),
    AmneziaProfileParameters => ("profile.amnezia_parameters", "Параметры AmneziaWG", "AmneziaWG settings"),
    ProfileSourceMissing => ("profile.source_missing", "Исходник этого импорта не сохранён. Вставьте ссылку или JSON одного сервера.", "The original import was not saved. Paste a link or JSON for one server."),
    HideServerSettings => ("desktop.hide_server_settings", "Скрыть настройки сервера", "Hide server settings"),
    HideServerSettingsHelp => ("desktop.hide_server_settings_help", "Управляет доступом к настройкам серверов подписки.", "Controls access to server settings within the subscription."),
    EncryptedSubscription => ("desktop.encrypted_subscription", "Зашифрованная подписка", "Encrypted subscription"),
    EncryptedSubscriptionHelp => ("desktop.encrypted_subscription_help", "Отмечает зашифрованную ссылку.", "Indicates an encrypted link."),
    AllowUnsafeConnection => ("desktop.allow_unsafe_connection", "Разрешить небезопасное подключение", "Allow insecure connection"),
    AllowUnsafeConnectionHelp => ("desktop.allow_unsafe_connection_help", "Разрешает незашифрованное получение подписки.", "Allows retrieving the subscription over an unencrypted connection."),
    Statistics => ("desktop.statistics", "Статистика", "Statistics"),
    StatsServer => ("desktop.stats_server", "Сервер", "Server"),
    StatsConnection => ("desktop.stats_connection", "Подключение", "Connection"),
    StatsNotConnected => ("desktop.stats_not_connected", "Не подключён", "Not connected"),
    StatsStartedAt => ("desktop.stats_started_at", "Время начала", "Start time"),
    StatsDuration => ("desktop.stats_duration", "Время подключения", "Connection duration"),
    StatsProxyBandwidth => ("desktop.stats_proxy_bandwidth", "Пропускная способность прокси", "Proxy bandwidth"),
    StatsProxyUsage => ("desktop.stats_proxy_usage", "Использование данных через прокси", "Proxy data usage"),
    StatsDirectUsage => ("desktop.stats_direct_usage", "Прямое использование данных", "Direct data usage"),
    StatsDownload => ("desktop.stats_download", "Загрузка", "Download"),
    StatsUpload => ("desktop.stats_upload", "Выгрузка", "Upload"),
    StatsUnavailable => ("desktop.stats_unavailable", "Данные недоступны в этой сборке.", "No data in this build."),
    LogCore => ("desktop.log_core", "Ядро", "Core"),
    LogReportHint => ("desktop.log_report_hint", "Нажмите Ctrl+R, чтобы открыть диагностический отчёт", "Press Ctrl+R to open a diagnostic report"),
    LogActions => ("desktop.log_actions", "Действия с логами", "Log actions"),
    LogCopy => ("desktop.log_copy", "Копировать выбранный лог", "Copy selected log"),
    LogEmpty => ("desktop.log_empty", "В этом логе пока нет событий", "This log has no events yet"),
    LogReport => ("desktop.log_report", "Диагностический отчёт", "Diagnostic report"),
    LogReportCopy => ("desktop.log_report_copy", "Копировать отчёт", "Copy report"),
    LogCategoryApplication => ("desktop.log_category_application", "Приложение", "Application"),
    LogCategoryConnection => ("desktop.log_category_connection", "Подключение", "Connection"),
    LogCategorySubscriptions => ("desktop.log_category_subscriptions", "Подписки", "Subscriptions"),
    LogCategoryProfiles => ("desktop.log_category_profiles", "Профили", "Profiles"),
    LogCategorySettings => ("desktop.log_category_settings", "Настройки", "Settings"),
    LogCategoryError => ("desktop.log_category_error", "Ошибка", "Error"),
    LogSettingsSaved => ("desktop.log_settings_saved", "Настройки сохранены", "Settings saved"),
    AboutVersion => ("desktop.about_version", "Версия программы", "App version"),
    AboutCoreVersion => ("desktop.about_core_version", "Ожидаемая версия BlazeCore", "Expected BlazeCore version"),
    AboutDevice => ("desktop.about_device", "Об устройстве", "Device info"),
    AboutRepository => ("desktop.about_repository", "Репозиторий", "Repository"),
    AboutLicenses => ("desktop.about_licenses", "Лицензии сторонних компонентов", "Third-Party Licenses"),
    AboutShow => ("desktop.about_show", "Показать", "Show"),
    AboutLicensesDesktop => ("desktop.about_licenses_desktop", "Библиотеки интерфейса", "Desktop UI libraries"),
    AboutUpdateTitle => ("desktop.about_update_title", "Доступна версия {version}", "Version {version} is available"),
    AboutUpdateCheck => ("desktop.about_update_check", "Проверить", "Check"),
    AboutUpdateCurrent => ("desktop.about_update_current", "Установлена актуальная версия", "You have the latest version"),
    AboutUpdateFailed => ("desktop.about_update_failed", "Не удалось выполнить обновление", "Could not complete the update"),
    AboutUpdateCancelled => ("desktop.about_update_cancelled", "Установка отменена (код {code}). Приложение продолжает работать.", "Installation cancelled (code {code}). The application remains open."),
    AboutUpdateUnpublished => ("desktop.about_update_unpublished", "Выпусков этого канала пока нет", "No releases are published for this channel yet"),
    AboutUpdateInstalling => ("desktop.about_update_installing", "Открыт мастер установки. Приложение останется открытым до подтверждения установки.", "The setup wizard is open. The application stays open until installation is confirmed."),
    AboutUpdateManaged => ("desktop.about_update_managed", "Для установки обновлений используйте Blazer, установленный через мастер установки. Установщик доступен на странице выпуска.", "To install updates, use Blazer installed by the setup wizard. The installer is available on the release page."),
    AboutUpdateDownloading => ("desktop.about_update_downloading", "Скачивание и проверка пакета", "Downloading and verifying the package"),
    AboutUpdateReady => ("desktop.about_update_ready", "Версия {version} готова к установке", "Version {version} is ready to install"),
    AboutUpdateInstall => ("desktop.about_update_install", "Установить и перезапустить", "Install and restart"),
    AboutUpdateDisconnect => ("desktop.about_update_disconnect", "Отключите VPN и дождитесь завершения операции", "Disconnect the VPN and wait for the current operation to finish"),
    AboutUpdateUnavailable => ("desktop.about_update_unavailable", "Источник обновлений не настроен", "No update feed is configured"),
    AboutUpdateOpenRelease => ("desktop.about_update_open_release", "Открыть выпуск", "Open release"),
    AboutUpdatePreview => ("desktop.about_update_preview", "Обновление интерфейса и навигации.", "Interface and navigation updates."),
    AboutUpdateSkip => ("desktop.about_update_skip", "Пропустить", "Skip"),
    AboutUpdateDownload => ("desktop.about_update_download", "Загрузка", "Download"),
    AppName => ("app.name", "Blazer", "Blazer"),
    DemoAppName => ("app.demo_name", "Blazer · Демо", "Blazer · Demo"),
    Version => ("app.version", "Версия {version}", "Version {version}"),
    Home => ("nav.home", "Главная", "Home"),
    Subscriptions => ("nav.subscriptions", "Подписки", "Subscriptions"),
    Profiles => ("nav.profiles", "Конфигурации", "Configurations"),
    Routing => ("nav.routing", "Маршрутизация", "Routing"),
    Vpn => ("nav.vpn", "VPN", "VPN"),
    Logs => ("nav.logs", "Логи", "Logs"),
    Settings => ("nav.settings", "Настройки", "Settings"),
    GeneralSettings => ("settings.general", "Общие", "General"),
    NetworkSettings => ("settings.network", "Настройки сети", "Network settings"),
    About => ("settings.about", "О программе", "About"),
    AboutCompact => ("settings.about.compact", "О Blazer", "About"),
    SettingsApplicationGroup => ("settings.group.application", "Приложение", "Application"),
    SettingsConnectionGroup => ("settings.group.connection", "Подключение", "Connection"),
    SettingsUpdateGroup => ("settings.group.update", "Обновление", "Update"),
    SettingsLaunchGroup => ("settings.group.launch", "При запуске", "On launch"),
    ManageSubscriptions => ("settings.subscriptions.manage", "Управление подписками", "Manage subscriptions"),
    AutoUpdate => ("settings.subscriptions.auto_update", "Автообновление", "Auto update"),
    UpdateInterval => ("settings.subscriptions.interval", "Интервал обновления", "Update interval"),
    UpdateOnLaunch => ("settings.subscriptions.on_launch", "Обновить при открытии", "Update on open"),
    PingOnLaunch => ("settings.subscriptions.ping_on_launch", "Пинг при открытии", "Ping on open"),
    ConnectOnLaunch => ("settings.subscriptions.connect_on_launch", "Подключаться при открытии", "Connect on open"),
    PingMethod => ("settings.ping.method", "Метод проверки", "Probe method"),
    PingDisplay => ("settings.ping.display", "Результат пинга", "Ping result"),
    SettingUnavailable => ("settings.unavailable", "Недоступно", "Unavailable"),
    SubscriptionSettings => ("settings.subscription", "Настройки подписки", "Subscription settings"),
    FallbackUrl => ("settings.subscription.fallback_url", "Резервный адрес", "Fallback URL"),
    FallbackConfigured => ("settings.subscription.fallback_configured", "Настроен", "Configured"),
    FallbackNotSet => ("settings.subscription.fallback_not_set", "Не задан", "Not set"),
    RemoveFallback => ("settings.subscription.remove_fallback", "Удалить резервный адрес", "Remove fallback URL"),
    ProfileHidden => ("configuration.hidden", "Детали конфигурации скрыты поставщиком", "The provider has hidden configuration details"),
    FallbackUsed => ("subscription.fallback_used", "Последнее обновление прошло через резервный адрес.", "The last update used the fallback URL."),
    ProviderProposedFallback => ("subscription.proposed_fallback", "Поставщик предложил резервный адрес", "Provider proposed a fallback URL"),
    ProviderProposedMigration => ("subscription.proposed_migration", "Поставщик предложил сменить основной адрес", "Provider proposed moving the primary URL"),
    ProviderOrigin => ("subscription.provider_origin", "Текущий домен: {origin}", "Current origin: {origin}"),
    ProviderTarget => ("subscription.provider_target", "Новый домен: {origin}", "New origin: {origin}"),
    ProviderApprovalHelp => ("subscription.provider_approval_help", "После разрешения при обновлении на новый домен может быть отправлена полная ссылка подписки, включая секрет.", "Allowing this change may send the full subscription URL, including its secret, to the new origin on refresh."),
    AllowProviderChange => ("subscription.provider_allow", "Разрешить", "Allow"),
    RejectProviderChange => ("subscription.provider_reject", "Отклонить", "Reject"),
    Back => ("nav.back", "Назад", "Back"),
    Mode => ("connection.mode", "Режим", "Mode"),
    ModeProxy => ("connection.mode.proxy", "Локальный прокси", "Local proxy"),
    ModeSystemProxy => ("connection.mode.system_proxy", "Системный прокси", "System proxy"),
    ModeTun => ("connection.mode.tun", "TUN", "TUN"),
    ModeMixed => ("connection.mode.mixed", "Смешанный", "Mixed"),
    ModeUnavailable => ("connection.mode.unavailable", "Этот режим недоступен в текущей сборке", "This mode is unavailable in this build"),
    ModeFinishFirst => ("connection.mode.finish_first", "Завершите подключение перед сменой режима", "Disconnect before changing modes"),
    ModeSaving => ("connection.mode.saving", "Сохранение режима…", "Saving mode…"),
    ActiveMode => ("connection.mode.active", "Активный режим: {mode}", "Active mode: {mode}"),
    LocalDns => ("connection.local_dns", "Использовать локальный DNS", "Use local DNS"),
    HideGroups => ("subscription.collapse", "Свернуть всё", "Collapse all"),
    ShowGroups => ("subscription.expand", "Развернуть всё", "Expand all"),
    HideGroup => ("subscription.collapse_one", "Свернуть группу", "Collapse group"),
    ShowGroup => ("subscription.expand_one", "Развернуть группу", "Expand group"),
    Ping => ("subscription.ping", "Пинг", "Ping"),
    SubscriptionActions => ("subscription.actions", "Действия с подпиской", "Subscription actions"),
    ProfileActions => ("configuration.actions", "Действия с конфигурацией", "Configuration actions"),
    Language => ("settings.language", "Язык", "Language"),
    Russian => ("settings.russian", "Русский", "Русский"),
    English => ("settings.english", "English", "English"),
    Connect => ("action.connect", "Подключиться", "Connect"),
    StopProxy => ("action.stop_proxy", "Остановить прокси", "Stop proxy"),
    Disconnect => ("action.disconnect", "Отключиться", "Disconnect"),
    Cancel => ("action.cancel", "Отменить", "Cancel"),
    Add => ("action.add", "Добавить", "Add"),
    Remove => ("action.remove", "Удалить", "Remove"),
    Refresh => ("action.refresh", "Обновить", "Refresh"),
    SubscriptionRefreshErrorTitle => ("subscription.refresh_error_title", "Не удалось обновить подписку", "Could not refresh subscription"),
    Select => ("action.select", "Выбрать", "Select"),
    Import => ("action.import", "Импортировать", "Import"),
    Close => ("action.close", "Закрыть", "Close"),
    Clear => ("action.clear", "Очистить", "Clear"),
    Selected => ("profile.selected", "Выбран", "Selected"),
    Disconnected => ("connection.disconnected", "Отключён", "Disconnected"),
    Connecting => ("connection.connecting", "Подключение…", "Connecting…"),
    ProxyRunning => ("connection.proxy_running", "Локальный прокси запущен", "Local proxy is running"),
    SystemProxyRunning => ("connection.system_proxy_running", "Системный прокси включён", "System proxy is enabled"),
    TunRunning => ("connection.tun_running", "TUN подключён", "TUN is connected"),
    RetryDisconnect => ("connection.retry_disconnect", "Повторить отключение", "Retry disconnect"),
    CleanupRequired => ("connection.cleanup_required", "Отключение не завершено. Повторите попытку.", "Disconnect did not finish. Retry it."),
    Disconnecting => ("connection.disconnecting", "Отключение…", "Disconnecting…"),
    Failed => ("connection.failed", "Не удалось подключиться", "Connection failed"),
    LocalProxy => ("mode.local_proxy", "Режим: локальный прокси", "Mode: local proxy"),
    SocksEndpoint => ("connection.socks_endpoint", "SOCKS5: {endpoint}", "SOCKS5: {endpoint}"),
    HttpEndpoint => ("connection.http_endpoint", "HTTP: {endpoint}", "HTTP: {endpoint}"),
    CombinedEndpoint => ("connection.combined_endpoint", "SOCKS5 и HTTP: {endpoint}", "SOCKS5 and HTTP: {endpoint}"),
    LocalProxyProtocols => ("connection.local_proxy_protocols", "Локальный прокси поддерживает SOCKS5 и HTTP.", "The local proxy supports SOCKS5 and HTTP."),
    NotSystemVpn => ("mode.not_system_vpn", "Это подключение работает только как локальный прокси; системные настройки VPN не изменяются. Настройте прокси в нужном приложении.", "This connection is a local proxy only; it does not change system VPN settings. Configure the proxy in the application you want to use."),
    SelectProfile => ("connection.select_profile", "Выберите или добавьте сервер", "Select or add a server"),
    ActiveProfile => ("connection.active_profile", "Выбранный сервер", "Selected server"),
    EngineUnavailable => ("error.engine_unavailable", "Ядро BlazeCore недоступно. Проверьте установку и настройки ядра.", "BlazeCore is unavailable. Check the engine installation and settings."),
    Unsupported => ("error.unsupported", "Эта возможность пока не поддерживается", "This feature is not supported yet"),
    Busy => ("error.busy", "Очередь операций занята. Повторите после завершения текущей операции.", "The operation queue is busy. Try again after the current operation finishes."),
    BackendStopped => ("error.backend_stopped", "Backend остановлен. Перезапустите приложение.", "The backend has stopped. Restart the application."),
    Working => ("state.working", "Подождите…", "Please wait…"),
    ErrorTitle => ("error.title", "Не удалось выполнить действие", "The action could not be completed"),
    NoSubscriptions => ("subscription.empty", "Добавьте подписку или импортируйте конфигурацию", "Add a subscription or import a configuration"),
    AddSubscription => ("subscription.add", "Добавить подписку", "Add subscription"),
    SubscriptionName => ("subscription.name", "Имя подписки", "Subscription name"),
    SubscriptionAutomaticName => ("subscription.automatic_name", "Пустое поле — имя от провайдера или имя хоста.", "Leave blank to use the provider title or host name."),
    SubscriptionPinned => ("subscription.pinned", "Подписка закреплена", "Subscription pinned"),
    SubscriptionAddress => ("subscription.address", "Основной адрес", "Primary address"),
    SubscriptionUrl => ("subscription.url", "URL подписки", "Subscription URL"),
    SubscriptionUrlHint => ("subscription.url_hint", "https://provider.example/subscription", "https://provider.example/subscription"),
    ProfileCount => ("subscription.profile_count", "Серверов: {count}", "Servers: {count}"),
    SubscriptionFailed => ("subscription.failed", "Подписка не обновлена. Предыдущие данные сохранены.", "The subscription was not updated. Previous data was kept."),
    SubscriptionDisconnectFirst => ("subscription.disconnect_first", "Сначала отключитесь от сервера этой подписки.", "Disconnect from this subscription’s server first."),
    RemoveSubscriptionConfirm => ("subscription.remove_confirm", "Удалить подписку и принадлежащие ей конфигурации?", "Remove this subscription and its configurations?"),
    ConfirmRemove => ("action.confirm_remove", "Да, удалить", "Yes, remove"),
    ManualImport => ("configuration.import", "Импорт конфигурации", "Import configuration"),
    ConfigurationName => ("configuration.name", "Название конфигурации", "Configuration name"),
    ConfigurationText => ("configuration.text", "Текст или ссылки конфигураций", "Configuration text or share links"),
    ConfigurationOpenFile => ("configuration.open_file", "Открыть файл", "Open file"),
    NoProfiles => ("configuration.empty", "Конфигурации пока не добавлены", "No configurations have been added"),
    ProfileProtocol => ("configuration.protocol", "Протокол: {protocol}", "Protocol: {protocol}"),
    ProfileServer => ("configuration.server", "Сервер: {server}", "Server: {server}"),
    AllProfiles => ("configuration.all", "Все конфигурации", "All configurations"),
    ProfileRemoved => ("configuration.remove_confirm", "Удалить эту конфигурацию?", "Remove this configuration?"),
    RoutingUnavailable => ("routing.unavailable", "Редактирование маршрутов ещё не поддерживается backend. Правила не изменены.", "Routing edits are not supported by the backend yet. No rules have been changed."),
    RoutingApply => ("routing.apply", "Изменения маршрутизации применяются только после подтверждения backend.", "Routing changes are applied only after backend confirmation."),
    VpnUnavailable => ("vpn.unavailable", "Наше приложение пока не управляет системным TUN/VPN на этой платформе.", "This application does not manage system TUN/VPN on this platform yet."),
    Diagnostics => ("logs.title", "Локальная диагностика", "Local diagnostics"),
    DiagnosticsCount => ("logs.count", "Событий в памяти: {count}; вытеснено: {dropped}", "Buffered events: {count}; dropped: {dropped}"),
    DiagnosticsPrivacy => ("logs.privacy", "Сетевые адреса, подписки и секреты в журнал не записываются.", "Network addresses, subscriptions and secrets are not written to the log."),
    Telemetry => ("telemetry.title", "Телеметрия", "Telemetry"),
    OpenTelemetry => ("desktop.settings.opentelemetry", "Телеметрия", "Telemetry"),
    TelemetryExportMode => ("desktop.settings.opentelemetry.mode", "Отправка", "Sending"),
    TelemetryDisabledMode => ("desktop.settings.opentelemetry.disabled", "Отключён", "Disabled"),
    TelemetryManualMode => ("desktop.settings.opentelemetry.manual", "Вручную (HTTP или HTTPS)", "Manual (HTTP or HTTPS)"),
    TelemetryEndpoint => ("desktop.settings.opentelemetry.endpoint", "Адрес экспорта OTLP", "OTLP endpoint"),
    TelemetryBearer => ("desktop.settings.opentelemetry.bearer", "Токен Bearer", "Bearer token"),
    TelemetryBearerConfigured => ("desktop.settings.opentelemetry.bearer_configured", "Токен сохранён. Пустое поле оставит его для прежнего URL. Для нового URL введите новый токен, если он нужен.", "Token saved. Leave the field blank to keep it for the same URL. Enter a new token if the new URL requires one."),
    TelemetryClearBearer => ("desktop.settings.opentelemetry.clear_bearer", "Удалить сохранённый токен", "Remove saved token"),
    TelemetryConfigured => ("desktop.settings.opentelemetry.configured", "Отправка настроена.", "Telemetry is configured."),
    TelemetryDeploymentConfigured => ("desktop.settings.opentelemetry.deployment_configured", "Действуют параметры запуска. Сохранение применит выбранную настройку.", "Launch configuration is active. Save to apply the selected setting."),
    TelemetryCheckAndSave => ("desktop.settings.opentelemetry.check_save", "Проверить и сохранить", "Check and save"),
    TelemetryChecking => ("desktop.settings.opentelemetry.checking", "Проверяем соединение с коллектором…", "Checking the connection to the collector…"),
    TelemetryCheckPassed => ("desktop.settings.opentelemetry.check_passed", "Коллектор принял проверочный запрос OTLP. Настройки сохранены.", "The collector accepted the OTLP test request. Settings saved."),
    TelemetryEndpointUnavailable => ("error.telemetry_endpoint", "Коллектор не принял проверочный запрос OTLP. Проверьте URL, токен и доступ к сети.", "The collector did not accept the OTLP test request. Check the URL, token, and network access."),
    TelemetryDisabled => ("telemetry.disabled", "Внешний экспорт отключён и не настроен.", "External export is disabled and not configured."),
    TelemetryLocal => ("telemetry.local", "Диагностика хранится в ограниченном локальном буфере.", "Diagnostics are kept in a bounded local buffer."),
    NoPersistentError => ("state.no_error", "Ошибок нет", "No errors"),
    InvalidInput => ("error.invalid_input", "Проверьте название, адрес и формат входных данных.", "Check the name, URL and input format."),
    SubscriptionHttpFailed => ("error.subscription_http", "Сервер подписки вернул ошибку HTTP.", "The subscription server returned an HTTP error."),
    DownloadFailed => ("error.download", "Не удалось загрузить подписку. Проверьте сеть и повторите.", "Could not download the subscription. Check the network and retry."),
    ParseFailed => ("error.parse", "Не удалось прочитать конфиг. Скопируйте оригинал и повторите импорт.", "The configuration could not be read. Copy the original and try importing again."),
    StoreFailed => ("error.store", "Не удалось сохранить данные. Изменение не подтверждено.", "Could not save the data. The change was not confirmed."),
    EngineFailed => ("error.engine", "Ошибка ядра. Проверьте доступность и совместимость профиля.", "Engine error. Check availability and profile compatibility."),
    ProfileRequired => ("error.profile_required", "Сначала выберите конфигурацию", "Select a configuration first"),
    NetworkTimeout => ("error.timeout", "Время ожидания истекло. Повторите операцию.", "The operation timed out. Try again."),
    InputTooLarge => ("error.too_large", "Входные данные превышают допустимый размер.", "The input exceeds the allowed size."),
    SubscriptionMissing => ("error.subscription_missing", "Подписка больше не существует. Обновите список.", "The subscription no longer exists. Refresh the list."),
    ProfileMissing => ("error.profile_missing", "Конфигурация больше не существует. Выберите другую.", "The configuration no longer exists. Select another one."),
    UpdateAvailable => ("state.saved", "Изменение сохранено", "The change was saved"),
    SearchProfiles => ("configuration.search", "Поиск конфигурации", "Search configurations"),
    SourceManual => ("configuration.manual", "Добавлено вручную", "Added manually"),
    TlsFailed => ("error.tls", "Не удалось проверить защищённое соединение с сервером подписки.", "Could not validate the secure connection to the subscription server."),
    StorageUnavailable => ("error.storage_unavailable", "Защищённое хранилище недоступно.", "Secure storage is unavailable."),
    StorageCorrupt => ("error.storage_corrupt", "Данные хранилища повреждены. Восстановление требует отдельного действия.", "Stored data is corrupt. Recovery requires a separate action."),
    EngineInvalid => ("error.engine_invalid", "Не удалось проверить конфигурацию профиля в BlazeCore.", "BlazeCore could not validate the profile configuration."),
    EngineReadiness => ("error.engine_readiness", "Ядро не подтвердило готовность подключения.", "The engine did not confirm connection readiness."),
    SystemProxyUnavailable => ("error.system_proxy_unavailable", "Не удалось включить системный прокси Windows. Проверьте доступ к настройкам прокси и другие работающие VPN-клиенты.", "Could not enable the Windows system proxy. Check access to proxy settings and other running VPN clients."),
    SystemProxyRestoreFailed => ("error.system_proxy_restore_failed", "Не удалось восстановить настройки прокси Windows. Повторите отключение.", "Could not restore Windows proxy settings. Try disconnecting again."),
    TunServiceUnavailable => ("error.tun_service_unavailable", "Запустите или установите службу BlazeCoreTun для вашей учётной записи Windows.", "Start or install the BlazeCoreTun service for your Windows account."),
    LinuxTunUnavailable => ("error.linux_tun_unavailable", "Для TUN в Linux нужны /dev/net/tun, nftables и право CAP_NET_ADMIN для BlazeCore и его дочерних процессов.", "Linux TUN requires /dev/net/tun, nftables and CAP_NET_ADMIN for BlazeCore and its child processes."),
    EngineStopped => ("error.engine_stopped", "Ядро остановилось. Подключение больше не активно.", "The engine stopped. The connection is no longer active."),
    Cancelled => ("error.cancelled", "Операция отменена", "The operation was cancelled"),
    EventLine => ("logs.event", "{sequence}: {event}", "{sequence}: {event}"),
    EventStarted => ("logs.started", "Backend запущен", "Backend started"),
    EventSubscriptionUpdated => ("logs.subscription_updated", "Подписка обновлена", "Subscription updated"),
    EventSubscriptionRemoved => ("logs.subscription_removed", "Подписка удалена", "Subscription removed"),
    EventProfilesImported => ("logs.profiles_imported", "Конфигурации импортированы", "Configurations imported"),
    EventProfileUpdated => ("logs.profile_updated", "Конфигурация обновлена", "Configuration updated"),
    EventProfileSelected => ("logs.profile_selected", "Конфигурация выбрана", "Configuration selected"),
    EventProfileRemoved => ("logs.profile_removed", "Конфигурация удалена", "Configuration removed"),
    Edit => ("action.edit", "Изменить", "Edit"),
    Save => ("action.save", "Сохранить", "Save"),
    DisconnectFirst => ("error.disconnect_first", "Сначала остановите активный прокси, затем измените профиль или подписку.", "Stop the active proxy before changing this profile or subscription."),
    ShutdownTimeout => ("error.shutdown_timeout", "Не удалось подтвердить завершение ядра за отведённое время.", "Engine shutdown could not be confirmed within the timeout."),
    EngineUntrusted => ("error.engine_untrusted", "Ядро не соответствует доверенной версии или контрольной сумме. Подключение отклонено.", "The engine does not match the trusted version or checksum. Connection was rejected."),
    UnsupportedLanguage => ("error.language", "Этот язык не поддерживается; выбран English.", "This language is not supported; English was selected.")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    MissingArgument(String),
    InvalidTemplate,
}

/// Named substitutions are inserted once and never interpreted as another template.
pub fn format(locale: Locale, key: Key, arguments: &[(&str, &str)]) -> Result<String, FormatError> {
    let mut remaining = text(locale, key);
    let mut result = String::new();
    while let Some(start) = remaining.find('{') {
        result.push_str(&remaining[..start]);
        let after_start = &remaining[start + 1..];
        let Some(end) = after_start.find('}') else {
            return Err(FormatError::InvalidTemplate);
        };
        let name = &after_start[..end];
        let value = arguments
            .iter()
            .find_map(|(argument, value)| (*argument == name).then_some(*value))
            .ok_or_else(|| FormatError::MissingArgument(name.to_owned()))?;
        result.push_str(value);
        remaining = &after_start[end + 1..];
    }
    result.push_str(remaining);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn parameters(template: &str) -> BTreeSet<&str> {
        template
            .split('{')
            .skip(1)
            .filter_map(|part| part.split_once('}').map(|(name, _)| name))
            .collect()
    }

    #[test]
    fn every_typed_key_exists_in_both_locales_with_equal_parameters() {
        let mut ids = BTreeSet::new();
        for key in Key::ALL {
            assert!(ids.insert(key.id()), "duplicate key: {}", key.id());
            let russian = text(Locale::Russian, *key);
            let english = text(Locale::English, *key);
            assert!(!russian.trim().is_empty(), "missing Russian: {}", key.id());
            assert!(!english.trim().is_empty(), "missing English: {}", key.id());
            assert_eq!(parameters(russian), parameters(english), "{}", key.id());
            assert_eq!(lookup(Locale::Russian, key.id()), russian);
            assert_eq!(lookup(Locale::English, key.id()), english);
        }
    }

    #[test]
    fn unknown_locale_has_deterministic_english_fallback() {
        assert_eq!(Locale::from_tag("missing"), Locale::English);
        assert_eq!(Locale::from_tag("ru"), Locale::Russian);
        assert_eq!(Locale::from_tag("en"), Locale::English);
    }

    #[test]
    fn missing_dynamic_key_is_visible_instead_of_panicking() {
        assert_eq!(lookup(Locale::Russian, "missing.key"), "missing.key");
    }

    #[test]
    fn named_arguments_preserve_user_content_without_recursive_expansion() {
        let value = "user-{count}-profile";
        assert_eq!(
            format(Locale::English, Key::ProfileServer, &[("server", value)]),
            Ok("Server: user-{count}-profile".to_owned())
        );
        assert_eq!(
            format(Locale::Russian, Key::ProfileCount, &[("count", "3")]),
            Ok("Серверов: 3".to_owned())
        );
    }

    #[test]
    fn missing_format_argument_is_reported() {
        assert_eq!(
            format(Locale::English, Key::ProfileCount, &[]),
            Err(FormatError::MissingArgument("count".to_owned()))
        );
    }
}
