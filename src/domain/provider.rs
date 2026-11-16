//! Subscription-provider metadata and private endpoints.
use std::collections::BTreeMap;

use crate::client::{ClientError, Secret};

pub const MAX_METADATA_BYTES: usize = 64 * 1024;
pub const MAX_METADATA_VALUE_BYTES: usize = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderMetadataClass {
    Neutral,
    Display,
    Behavior,
}

macro_rules! provider_keys {
    ($($variant:ident => ($wire:literal, $class:ident)),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        pub enum ProviderKey { $($variant),+ }

        impl ProviderKey {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub const fn wire_name(self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }

            pub fn from_wire(name: &str) -> Option<Self> {
                Self::ALL
                    .iter()
                    .copied()
                    .find(|key| key.wire_name().eq_ignore_ascii_case(name))
            }

            pub const fn class(self) -> ProviderMetadataClass {
                match self { $(Self::$variant => ProviderMetadataClass::$class),+ }
            }
        }
    };
}

provider_keys! {
    // Keep reading this field from existing sealed catalogs and wire responses.
    ProviderId => ("providerid", Neutral),
    ProfileUpdateInterval => ("profile-update-interval", Behavior),
    ProfileTitle => ("profile-title", Neutral),
    Name => ("name", Neutral),
    SubscriptionUserinfo => ("subscription-userinfo", Neutral),
    SupportUrl => ("support-url", Neutral),
    ProfileWebPageUrl => ("profile-web-page-url", Neutral),
    Announce => ("announce", Neutral),
    RoutingEnable => ("routing-enable", Behavior),
    CustomTunnelConfig => ("custom-tunnel-config", Behavior),
    SocksAuthMode => ("socks-auth-mode", Behavior),
    SocksAuthUser => ("socks-auth-user", Behavior),
    SocksAuthPassword => ("socks-auth-password", Behavior),
    HttpAuthMode => ("http-auth-mode", Behavior),
    HttpAuthUser => ("http-auth-user", Behavior),
    HttpAuthPassword => ("http-auth-password", Behavior),
    TunType => ("tun-type", Behavior),
    NewUrl => ("new-url", Behavior),
    NewDomain => ("new-domain", Behavior),
    FallbackUrl => ("fallback-url", Behavior),
    SubInfoColor => ("sub-info-color", Display),
    SubInfoText => ("sub-info-text", Display),
    SubInfoButtonText => ("sub-info-button-text", Display),
    SubInfoButtonLink => ("sub-info-button-link", Display),
    SubExpire => ("sub-expire", Display),
    SubExpireButtonLink => ("sub-expire-button-link", Display),
    NoLimitEnabled => ("no-limit-enabled", Behavior),
    NoLimitXhttpEnabled => ("no-limit-xhttp-enabled", Behavior),
    SubscriptionAlwaysHwidEnable => ("subscription-always-hwid-enable", Behavior),
    NotificationSubsExpire => ("notification-subs-expire", Behavior),
    HideSettings => ("hide-settings", Display),
    ServerAddressResolveEnable => ("server-address-resolve-enable", Behavior),
    ServerAddressResolveDnsDomain => ("server-address-resolve-dns-domain", Behavior),
    ServerAddressResolveDnsIp => ("server-address-resolve-dns-ip", Behavior),
    SubscriptionAutoconnect => ("subscription-autoconnect", Behavior),
    SubscriptionAutoconnectType => ("subscription-autoconnect-type", Behavior),
    SubscriptionPingOnopenEnabled => ("subscription-ping-onopen-enabled", Behavior),
    SubscriptionAutoUpdateEnable => ("subscription-auto-update-enable", Behavior),
    FragmentationEnable => ("fragmentation-enable", Behavior),
    FragmentationPackets => ("fragmentation-packets", Behavior),
    FragmentationLength => ("fragmentation-length", Behavior),
    FragmentationInterval => ("fragmentation-interval", Behavior),
    FragmentationMaxsplit => ("fragmentation-maxsplit", Behavior),
    NoisesEnable => ("noises-enable", Behavior),
    NoisesPacketType => ("noises-packet-type", Behavior),
    NoisesPacket => ("noises-packet", Behavior),
    NoisesDelay => ("noises-delay", Behavior),
    NoisesRand => ("noises-rand", Behavior),
    NoisesRandRange => ("noises-rand-range", Behavior),
    PingType => ("ping-type", Behavior),
    CheckUrlViaProxy => ("check-url-via-proxy", Behavior),
    ChangeUserAgent => ("change-user-agent", Behavior),
    AppAutoStart => ("app-auto-start", Behavior),
    SubscriptionAutoUpdateOpenEnable => ("subscription-auto-update-open-enable", Behavior),
    PerAppProxyMode => ("per-app-proxy-mode", Behavior),
    PerAppProxyList => ("per-app-proxy-list", Behavior),
    PerAppProxyListInvert => ("per-app-proxy-list-invert", Behavior),
    PerAppProxyListSet => ("per-app-proxy-list-set", Behavior),
    SniffingEnable => ("sniffing-enable", Behavior),
    SubscriptionsCollapse => ("subscriptions-collapse", Display),
    SubscriptionsExpandNow => ("subscriptions-expand-now", Display),
    PingResult => ("ping-result", Display),
    MuxEnable => ("mux-enable", Behavior),
    MuxTcpConnections => ("mux-tcp-connections", Behavior),
    MuxXudpConnections => ("mux-xudp-connections", Behavior),
    MuxQuic => ("mux-quic", Behavior),
    ProxyEnable => ("proxy-enable", Behavior),
    TunEnable => ("tun-enable", Behavior),
    TunMode => ("tun-mode", Behavior),
    ExcludeRoutes => ("exclude-routes", Behavior),
    ExcludeRoutesSet => ("exclude-routes-set", Behavior),
    ColorProfile => ("color-profile", Display),
    IncludeAllNetworksEnable => ("include-all-networks-enable", Behavior),
    ExcludeLocalNetworksEnable => ("exclude-local-networks-enable", Behavior),
    ExcludeApnsEnable => ("exclude-apns-enable", Behavior),
    DontUseFilter => ("dont-use-filter", Display),
    SubscriptionPin => ("subscription-pin", Display),
    ManualBlockUserAgent => ("manual-block-user-agent", Behavior),
    SubscriptionsSortType => ("subscriptions-sort-type", Display),
    DnsFromJsonEnable => ("dns-from-json-enable", Behavior),
    UserAgentGeoFiles => ("user-agent-geo-files", Behavior),
    ProxyPingTimeout => ("proxy-ping-timeout", Behavior),
    HideVpnIcon => ("hide-vpn-icon", Behavior),
    SubscriptionRequestTimeout => ("subscription-request-timeout", Behavior),
    InboundHttpEnable => ("inbound-http-enable", Behavior),
    XrayTunEnable => ("xray-tun-enable", Behavior),
    XrayTunMtu => ("xray-tun-mtu", Behavior),
    BlockBindToTunnelEnable => ("block-bind-to-tunnel-enable", Behavior),
    SubscriptionAlternativeHwidEnabled => ("subscription-alternative-hwid-enabled", Behavior),
    ProxyPingMode => ("proxy-ping-mode", Behavior),
    PerAppProxyModeWindows => ("per-app-proxy-mode-windows", Behavior),
    PerAppProxyModeMacos => ("per-app-proxy-mode-macos", Behavior),
    PerAppProxyModeLinux => ("per-app-proxy-mode-linux", Behavior),
    PerAppProxyListWindowsAdd => ("per-app-proxy-list-windows-add", Behavior),
    PerAppProxyListWindowsSet => ("per-app-proxy-list-windows-set", Behavior),
    PerAppProxyListMacosAdd => ("per-app-proxy-list-macos-add", Behavior),
    PerAppProxyListMacosSet => ("per-app-proxy-list-macos-set", Behavior),
    PerAppProxyListLinuxAdd => ("per-app-proxy-list-linux-add", Behavior),
    PerAppProxyListLinuxSet => ("per-app-proxy-list-linux-set", Behavior),
    Routing => ("routing", Behavior),
    EncryptTag => ("encrypt-tag", Behavior),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderMetadata {
    pub values: BTreeMap<ProviderKey, Secret>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SubscriptionUsage {
    pub used_bytes: Option<u64>,
    /// Some(0) represents a provider that explicitly reports no quota limit.
    pub total_bytes: Option<u64>,
    pub expires_at_unix: Option<u64>,
}

impl ProviderMetadata {
    pub fn value(&self, key: ProviderKey) -> Option<&Secret> {
        self.values.get(&key)
    }

    pub fn enabled(&self, key: ProviderKey) -> bool {
        self.value(key).is_some_and(|value| {
            value.expose().eq_ignore_ascii_case("true") || value.expose() == "1"
        })
    }

    pub fn request_timeout(&self) -> u64 {
        self.value(ProviderKey::SubscriptionRequestTimeout)
            .and_then(|value| value.expose().parse().ok())
            .filter(|seconds| (5..=15).contains(seconds))
            .unwrap_or(9)
    }

    pub fn validate(&self) -> Result<(), ClientError> {
        let total = self.values.iter().try_fold(0, |total, (key, value)| {
            let value = value.expose();
            if value.len() > MAX_METADATA_VALUE_BYTES {
                return Err(ClientError::PayloadTooLarge);
            }
            if value.chars().any(|ch| ch.is_control() && ch != '\t') {
                return Err(ClientError::InvalidSubscription);
            }
            Ok(total + key.wire_name().len() + value.len())
        })?;
        if total > MAX_METADATA_BYTES {
            return Err(ClientError::PayloadTooLarge);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderEndpoint {
    pub url: Secret,
    pub origin: String,
}

impl ProviderEndpoint {
    pub fn same_request(&self, other: &Self) -> bool {
        self.url
            .expose()
            .split_once('#')
            .map_or(self.url.expose(), |(url, _)| url)
            == other
                .url
                .expose()
                .split_once('#')
                .map_or(other.url.expose(), |(url, _)| url)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderChangeKind {
    Fallback,
    Migration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingProviderChange {
    pub kind: ProviderChangeKind,
    pub nonce: String,
    pub from_origin: String,
    pub endpoint: ProviderEndpoint,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderState {
    pub metadata: ProviderMetadata,
    pub authority_origin: Option<String>,
    pub fallback: Option<ProviderEndpoint>,
    pub fallback_user_set: bool,
    pub pending: Vec<PendingProviderChange>,
    pub last_success_unix: Option<u64>,
    pub last_automatic_failure_unix: Option<u64>,
    pub last_update_used_fallback: bool,
    /// Opaque validator for the primary document from which the current profiles were created.
    pub cached_version: Option<Secret>,
}

impl ProviderState {
    /// A profile presentation policy within this subscription.
    pub fn raw_config_hidden(&self) -> bool {
        self.metadata.enabled(ProviderKey::HideSettings)
    }

    pub fn refresh_interval_hours(&self) -> Option<u32> {
        self.metadata
            .value(ProviderKey::ProfileUpdateInterval)?
            .expose()
            .parse::<u32>()
            .ok()
            .filter(|hours| *hours > 0)
    }

    pub fn refresh_on_launch(&self) -> bool {
        self.metadata
            .enabled(ProviderKey::SubscriptionAutoUpdateOpenEnable)
    }
}
