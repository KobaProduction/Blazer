use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vpn_application::client::StateStorePort;
use vpn_domain::client::{
    AlpnProtocols, Catalog, CatalogProfile, ClientError, ClientSettings, ConnectionMode, Language,
    ProfileUnavailableReason, ProxyProfile, ProxyProtocol, ProxySecurity, ProxyTransport, Secret,
    Subscription, TelemetryMode, TelemetrySettings, TlsFingerprint, UnavailableProfile,
    WebSocketEarlyData, XhttpMetadata, XhttpPacketOptions, XhttpPacketRange,
    validate_subscription_user_agent,
};
use vpn_domain::routing::{
    DnsPolicy, RouteAction, RouteRule, RouteTarget, RoutingAssignment, RoutingCatalog,
    RoutingInput, RoutingProfile,
};
use zeroize::Zeroizing;

use crate::provider_store::StoredProviderState;

mod probe;
use probe::ProbeSettingsDto;
mod graph;
use graph::GraphDto;

const HEADER: &[u8] = b"VPNC01\0";
const MAX_CATALOG_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENCRYPTED_BYTES: usize = MAX_CATALOG_BYTES + 65536;
const SUBSCRIPTION_DEVICE_ID_FILE: &str = "subscription-device-id";

#[cfg(target_os = "android")]
mod android_lock;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub mod sealing;
#[cfg(target_os = "android")]
use android_lock::CatalogLock;

#[cfg(not(target_os = "android"))]
type CatalogLock = fs::File;

pub struct UserSecretProtection;

pub trait CatalogProtection: Send {
    fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, ClientError>;
    fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, ClientError>;
}

impl CatalogProtection for UserSecretProtection {
    fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, ClientError> {
        protect(plaintext)
    }

    fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, ClientError> {
        unprotect(ciphertext)
    }
}

#[cfg(windows)]
fn protect(plaintext: &[u8]) -> Result<Vec<u8>, ClientError> {
    windows_dpapi::encrypt_data(plaintext, windows_dpapi::Scope::User, None)
        .map_err(|_| ClientError::StorageUnavailable)
}

#[cfg(windows)]
fn unprotect(ciphertext: &[u8]) -> Result<Vec<u8>, ClientError> {
    windows_dpapi::decrypt_data(ciphertext, windows_dpapi::Scope::User, None)
        .map_err(|_| ClientError::StorageCorrupt)
}

#[cfg(not(windows))]
fn protect(_plaintext: &[u8]) -> Result<Vec<u8>, ClientError> {
    Err(ClientError::StorageUnavailable)
}

#[cfg(not(windows))]
fn unprotect(_ciphertext: &[u8]) -> Result<Vec<u8>, ClientError> {
    Err(ClientError::StorageUnavailable)
}

pub struct SealedCatalogStore {
    directory: PathBuf,
    protection: Box<dyn CatalogProtection>,
    _writer_lock: CatalogLock,
}

impl SealedCatalogStore {
    pub fn new(directory: PathBuf) -> Result<Self, ClientError> {
        Self::with_protection(directory, Box::new(UserSecretProtection))
    }

    pub fn with_protection(
        directory: PathBuf,
        protection: Box<dyn CatalogProtection>,
    ) -> Result<Self, ClientError> {
        fs::create_dir_all(&directory).map_err(|_| ClientError::StorageUnavailable)?;
        let writer_lock = lock_catalog(&directory)?;
        Ok(Self {
            directory,
            protection,
            _writer_lock: writer_lock,
        })
    }

    fn path(&self) -> PathBuf {
        self.directory.join("catalog.sealed")
    }

    pub fn subscription_hwid(&self) -> Result<Secret, ClientError> {
        subscription_device_id(&self.directory)
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

/// The backend owns the catalog's writer lock while creating or reading the installation ID.
pub(crate) fn subscription_device_id(directory: &Path) -> Result<Secret, ClientError> {
    let path = directory.join(SUBSCRIPTION_DEVICE_ID_FILE);
    match fs::File::open(&path) {
        Ok(file) => read_subscription_device_id(file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let id = uuid::Uuid::new_v4().hyphenated().to_string();
            save_subscription_device_id(directory, &path, &id)?;
            Ok(Secret::new(id))
        }
        Err(_) => Err(ClientError::StorageUnavailable),
    }
}

fn read_subscription_device_id(file: fs::File) -> Result<Secret, ClientError> {
    let mut value = String::new();
    file.take(37)
        .read_to_string(&mut value)
        .map_err(|_| ClientError::StorageCorrupt)?;
    let id = uuid::Uuid::parse_str(&value).map_err(|_| ClientError::StorageCorrupt)?;
    if id.get_version() != Some(uuid::Version::Random) || value != id.hyphenated().to_string() {
        return Err(ClientError::StorageCorrupt);
    }
    Ok(Secret::new(value))
}

fn save_subscription_device_id(directory: &Path, path: &Path, id: &str) -> Result<(), ClientError> {
    let temporary = directory.join(format!(
        ".subscription-device-id.{}.pending",
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(id.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|_| ClientError::StorageWrite)
}

fn open_lock_file(directory: &Path) -> Result<fs::File, ClientError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("catalog.lock"))
        .map_err(|_| ClientError::StorageUnavailable)
}

#[cfg(not(target_os = "android"))]
fn lock_catalog(directory: &Path) -> Result<CatalogLock, ClientError> {
    let file = open_lock_file(directory)?;
    file.try_lock()
        .map_err(|_| ClientError::StorageUnavailable)?;
    Ok(file)
}

#[cfg(target_os = "android")]
fn lock_catalog(directory: &Path) -> Result<CatalogLock, ClientError> {
    CatalogLock::acquire(directory)
}

impl StateStorePort for SealedCatalogStore {
    fn load(&self) -> Result<Option<Catalog>, ClientError> {
        let mut file = match fs::File::open(self.path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ClientError::StorageUnavailable),
        };
        let mut ciphertext = Vec::new();
        Read::by_ref(&mut file)
            .take((MAX_ENCRYPTED_BYTES + 1) as u64)
            .read_to_end(&mut ciphertext)
            .map_err(|_| ClientError::StorageCorrupt)?;
        if ciphertext.len() > MAX_ENCRYPTED_BYTES || !ciphertext.starts_with(HEADER) {
            return Err(ClientError::StorageCorrupt);
        }
        let plaintext = Zeroizing::new(self.protection.unprotect(&ciphertext[HEADER.len()..])?);
        if plaintext.len() > MAX_CATALOG_BYTES {
            return Err(ClientError::StorageCorrupt);
        }
        let stored: StoredCatalog =
            serde_json::from_slice(&plaintext).map_err(|_| ClientError::StorageCorrupt)?;
        stored.into_catalog().map(Some)
    }

    fn save(&self, catalog: &Catalog) -> Result<(), ClientError> {
        catalog.settings.probe.validate()?;
        for item in &catalog.profiles {
            if let ProxyProtocol::Graph(graph) = &item.profile.protocol {
                graph.validate()?;
            }
        }
        let plaintext = Zeroizing::new(
            serde_json::to_vec(&StoredCatalog::from_catalog(catalog))
                .map_err(|_| ClientError::StorageWrite)?,
        );
        if plaintext.len() > MAX_CATALOG_BYTES {
            return Err(ClientError::PayloadTooLarge);
        }
        let ciphertext = self.protection.protect(&plaintext)?;
        if ciphertext.len() > MAX_ENCRYPTED_BYTES - HEADER.len() {
            return Err(ClientError::StorageWrite);
        }
        let temporary = self
            .directory
            .join(format!("catalog.{}.pending", uuid::Uuid::new_v4()));
        let result = write_encrypted(&temporary, &ciphertext).and_then(|()| {
            fs::rename(&temporary, self.path()).map_err(|_| ClientError::StorageWrite)
        });
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

fn write_encrypted(path: &Path, ciphertext: &[u8]) -> Result<(), ClientError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| ClientError::StorageWrite)?;
    file.write_all(HEADER)
        .and_then(|()| file.write_all(ciphertext))
        .and_then(|()| file.sync_all())
        .map_err(|_| ClientError::StorageWrite)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCatalog {
    version: u32,
    subscriptions: Vec<StoredSubscription>,
    profiles: Vec<StoredProfile>,
    selected_profile_uuid: Option<String>,
    language: String,
    local_proxy_port: u16,
    #[serde(default)]
    connection_mode: ModeDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    send_subscription_hwid: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subscription_user_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    telemetry: Option<TelemetrySettingsDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    probe: Option<ProbeSettingsDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    routing: Option<RoutingCatalogDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TelemetrySettingsDto {
    mode: TelemetryModeDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bearer_token: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TelemetryModeDto {
    Default,
    Disabled,
    Manual,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoutingCatalogDto {
    profiles: Vec<RoutingProfileDto>,
    default_profile_uuid: Option<String>,
    assignments: BTreeMap<String, RoutingAssignmentDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoutingProfileDto {
    uuid: String,
    revision: u64,
    title: String,
    rules: Vec<RouteRuleDto>,
    dns: DnsPolicyDto,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoutingAssignmentDto {
    profile_uuid: Option<String>,
    enabled: bool,
    ignore_provider: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteRuleDto {
    target: RouteTargetDto,
    action: RouteActionDto,
}

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum RouteTargetDto {
    Domain(String),
    DomainSuffix(String),
    Cidr(String),
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RouteActionDto {
    Proxy,
    Direct,
    Block,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum DnsPolicyDto {
    Automatic,
    System,
    Tcp {
        endpoint: SocketAddr,
    },
    Tls {
        endpoint: SocketAddr,
        server_name: String,
    },
    Https {
        endpoint: SocketAddr,
        server_name: String,
        path: String,
    },
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ModeDto {
    #[default]
    LocalProxy,
    SystemProxy,
    Tun,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSubscription {
    subscription_uuid: String,
    title: String,
    #[serde(default)]
    auto_title: bool,
    url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<StoredProviderState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    unavailable_profiles: Vec<StoredUnavailableProfile>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredUnavailableProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    probe: Option<StoredUnavailableProbe>,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    protocol: Option<String>,
    reason: UnavailableReasonDto,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredUnavailableProbe {
    uuid: String,
    host: String,
    port: u16,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum UnavailableReasonDto {
    Unsupported,
    UnsupportedXhttpUploadMethod,
    Invalid,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredProfile {
    profile_uuid: String,
    subscription_uuid: Option<String>,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    host: String,
    port: u16,
    protocol: ProtocolDto,
    transport: TransportDto,
    security: SecurityDto,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ProtocolDto {
    Graph {
        graph: Box<GraphDto>,
    },
    Native {
        settings: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        dns_servers: Vec<std::net::IpAddr>,
    },
    Socks5 {
        username: Option<String>,
        password: Option<String>,
    },
    Http {
        username: Option<String>,
        password: Option<String>,
    },
    Vless {
        user_uuid: String,
        vision: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encryption: Option<String>,
    },
    Vmess {
        user_uuid: String,
        cipher: String,
    },
    Trojan {
        password: String,
    },
    Hysteria2 {
        password: String,
        salamander_password: Option<String>,
        udp_enabled: bool,
    },
    Shadowsocks {
        cipher: String,
        password: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum TransportDto {
    Tcp,
    Udp,
    WebSocket {
        path: String,
        host: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        early_data: Option<WebSocketEarlyDataDto>,
    },
    Xhttp {
        path: String,
        host: Option<String>,
        mode: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<XhttpMetadataDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        packet: Option<XhttpPacketDto>,
    },
    Quic {
        certificate_sha256: Option<String>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct XhttpPacketDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_post_bytes: Option<(u32, u32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min_post_interval_ms: Option<(u32, u32)>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    wire: serde_json::Map<String, serde_json::Value>,
}

impl XhttpPacketDto {
    fn from_options(options: &XhttpPacketOptions) -> Option<Self> {
        (options != &XhttpPacketOptions::default()).then(|| Self {
            max_post_bytes: options.max_post_bytes.map(XhttpPacketRange::bounds),
            min_post_interval_ms: options.min_post_interval_ms.map(XhttpPacketRange::bounds),
            wire: crate::xhttp_wire::render(&options.wire),
        })
    }

    fn into_options(self) -> Result<XhttpPacketOptions, ClientError> {
        let range =
            |(min, max)| XhttpPacketRange::new(min, max).map_err(|_| ClientError::StorageCorrupt);
        Ok(XhttpPacketOptions {
            max_post_bytes: self.max_post_bytes.map(range).transpose()?,
            min_post_interval_ms: self.min_post_interval_ms.map(range).transpose()?,
            wire: crate::xhttp_wire::restore(&self.wire)?,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct XhttpMetadataDto {
    session_id_placement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id_key: Option<String>,
    seq_placement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    seq_key: Option<String>,
}

impl XhttpMetadataDto {
    fn from_metadata(value: &XhttpMetadata) -> Option<Self> {
        (value != &XhttpMetadata::default()).then(|| Self {
            session_id_placement: value.session_id_placement.name().to_owned(),
            session_id_key: value.session_id_key.clone(),
            seq_placement: value.seq_placement.name().to_owned(),
            seq_key: value.seq_key.clone(),
        })
    }

    fn into_metadata(self) -> Result<XhttpMetadata, ClientError> {
        let value = XhttpMetadata {
            session_id_placement: self
                .session_id_placement
                .parse()
                .map_err(|_| ClientError::StorageCorrupt)?,
            session_id_key: self.session_id_key,
            seq_placement: self
                .seq_placement
                .parse()
                .map_err(|_| ClientError::StorageCorrupt)?,
            seq_key: self.seq_key,
        };
        value.validate().map_err(|_| ClientError::StorageCorrupt)?;
        Ok(value)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WebSocketEarlyDataDto {
    max_bytes: NonZeroU16,
    header_name: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum SecurityDto {
    None,
    Tls {
        server_name: String,
        allow_insecure: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        alpn: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fingerprint: Option<String>,
    },
    Reality {
        server_name: String,
        public_key: String,
        short_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fingerprint: Option<String>,
    },
}

impl StoredCatalog {
    fn from_catalog(catalog: &Catalog) -> Self {
        Self {
            version: 1,
            subscriptions: catalog
                .subscriptions
                .iter()
                .map(|value| StoredSubscription {
                    subscription_uuid: value.subscription_uuid.clone(),
                    title: value.title.clone(),
                    auto_title: value.auto_title,
                    url: value.url.expose().to_owned(),
                    provider: Some(StoredProviderState::from_state(&value.provider)),
                    unavailable_profiles: value
                        .unavailable_profiles
                        .iter()
                        .map(StoredUnavailableProfile::from)
                        .collect(),
                })
                .collect(),
            profiles: catalog
                .profiles
                .iter()
                .map(StoredProfile::from_profile)
                .collect(),
            selected_profile_uuid: catalog.selected_profile_uuid.clone(),
            language: match catalog.settings.language {
                Language::Russian => "ru",
                Language::English => "en",
            }
            .to_owned(),
            local_proxy_port: catalog.settings.local_proxy_port,
            connection_mode: match catalog.settings.connection_mode {
                ConnectionMode::LocalProxy => ModeDto::LocalProxy,
                ConnectionMode::SystemProxy => ModeDto::SystemProxy,
                ConnectionMode::Tun => ModeDto::Tun,
            },
            send_subscription_hwid: (!catalog.settings.send_subscription_hwid).then_some(false),
            subscription_user_agent: catalog
                .settings
                .subscription_user_agent
                .as_ref()
                .map(|value| value.expose().to_owned()),
            telemetry: TelemetrySettingsDto::from_settings(&catalog.settings.telemetry),
            probe: ProbeSettingsDto::from_settings(&catalog.settings.probe),
            routing: RoutingCatalogDto::from_catalog(&catalog.routing),
        }
    }

    fn into_catalog(self) -> Result<Catalog, ClientError> {
        if self.version != 1 {
            return Err(ClientError::StorageCorrupt);
        }
        if let Some(value) = &self.subscription_user_agent {
            validate_subscription_user_agent(value).map_err(|_| ClientError::StorageCorrupt)?;
        }
        let language = match self.language.as_str() {
            "ru" => Language::Russian,
            "en" => Language::English,
            _ => Language::English,
        };
        Ok(Catalog {
            subscriptions: self
                .subscriptions
                .into_iter()
                .map(StoredSubscription::into_subscription)
                .collect::<Result<Vec<_>, _>>()?,
            profiles: self
                .profiles
                .into_iter()
                .map(StoredProfile::into_profile)
                .collect::<Result<Vec<_>, _>>()?,
            selected_profile_uuid: self.selected_profile_uuid,
            routing: self
                .routing
                .map(RoutingCatalogDto::into_catalog)
                .unwrap_or_default(),
            settings: ClientSettings {
                language,
                local_proxy_port: self.local_proxy_port,
                send_subscription_hwid: self.send_subscription_hwid.unwrap_or(true),
                subscription_user_agent: self.subscription_user_agent.map(Secret::new),
                telemetry: self
                    .telemetry
                    .map(TelemetrySettingsDto::into_settings)
                    .transpose()?
                    .unwrap_or_default(),
                probe: self
                    .probe
                    .map(ProbeSettingsDto::into_settings)
                    .transpose()?
                    .unwrap_or_default(),
                connection_mode: match self.connection_mode {
                    ModeDto::LocalProxy => ConnectionMode::LocalProxy,
                    ModeDto::SystemProxy => ConnectionMode::SystemProxy,
                    ModeDto::Tun => ConnectionMode::Tun,
                },
            },
        })
    }
}

impl TelemetrySettingsDto {
    fn from_settings(settings: &TelemetrySettings) -> Option<Self> {
        if settings == &TelemetrySettings::default() {
            return None;
        }
        Some(Self {
            mode: match settings.mode {
                TelemetryMode::Default => TelemetryModeDto::Default,
                TelemetryMode::Disabled => TelemetryModeDto::Disabled,
                TelemetryMode::Manual => TelemetryModeDto::Manual,
            },
            endpoint: settings.endpoint.clone(),
            bearer_token: settings
                .bearer_token
                .as_ref()
                .map(|token| token.expose().to_owned()),
        })
    }

    fn into_settings(self) -> Result<TelemetrySettings, ClientError> {
        let settings = TelemetrySettings {
            mode: match self.mode {
                TelemetryModeDto::Default => TelemetryMode::Default,
                TelemetryModeDto::Disabled => TelemetryMode::Disabled,
                TelemetryModeDto::Manual => TelemetryMode::Manual,
            },
            endpoint: self.endpoint,
            bearer_token: self.bearer_token.map(Secret::new),
        };
        settings
            .validate()
            .map_err(|_| ClientError::StorageCorrupt)?;
        if let Some(endpoint) = &settings.endpoint {
            crate::provider::endpoint(endpoint).map_err(|_| ClientError::StorageCorrupt)?;
        }
        Ok(settings)
    }
}

impl RoutingCatalogDto {
    fn from_catalog(catalog: &RoutingCatalog) -> Option<Self> {
        if catalog.profiles.is_empty()
            && catalog.default_profile_uuid.is_none()
            && catalog.assignments.is_empty()
        {
            return None;
        }
        Some(Self {
            profiles: catalog
                .profiles
                .iter()
                .map(RoutingProfileDto::from)
                .collect(),
            default_profile_uuid: catalog.default_profile_uuid.clone(),
            assignments: catalog
                .assignments
                .iter()
                .map(|(uuid, assignment)| {
                    (
                        uuid.clone(),
                        RoutingAssignmentDto {
                            profile_uuid: assignment.profile_uuid.clone(),
                            enabled: assignment.enabled,
                            ignore_provider: assignment.ignore_provider,
                        },
                    )
                })
                .collect(),
        })
    }

    fn into_catalog(self) -> RoutingCatalog {
        RoutingCatalog {
            profiles: self
                .profiles
                .into_iter()
                .map(RoutingProfile::from)
                .collect(),
            default_profile_uuid: self.default_profile_uuid,
            assignments: self
                .assignments
                .into_iter()
                .map(|(uuid, assignment)| {
                    (
                        uuid,
                        RoutingAssignment {
                            profile_uuid: assignment.profile_uuid,
                            enabled: assignment.enabled,
                            ignore_provider: assignment.ignore_provider,
                        },
                    )
                })
                .collect(),
        }
    }
}

impl From<&RoutingProfile> for RoutingProfileDto {
    fn from(profile: &RoutingProfile) -> Self {
        Self {
            uuid: profile.uuid.clone(),
            revision: profile.revision,
            title: profile.input.title.clone(),
            rules: profile.input.rules.iter().map(RouteRuleDto::from).collect(),
            dns: DnsPolicyDto::from(&profile.input.dns),
        }
    }
}

impl From<RoutingProfileDto> for RoutingProfile {
    fn from(profile: RoutingProfileDto) -> Self {
        Self {
            uuid: profile.uuid,
            revision: profile.revision,
            input: RoutingInput {
                title: profile.title,
                rules: profile.rules.into_iter().map(RouteRule::from).collect(),
                dns: DnsPolicy::from(profile.dns),
            },
        }
    }
}

impl From<&RouteRule> for RouteRuleDto {
    fn from(rule: &RouteRule) -> Self {
        let target = match &rule.target {
            RouteTarget::Domain(name) => RouteTargetDto::Domain(name.clone()),
            RouteTarget::DomainSuffix(name) => RouteTargetDto::DomainSuffix(name.clone()),
            RouteTarget::Cidr(cidr) => RouteTargetDto::Cidr(cidr.clone()),
        };
        let action = match rule.action {
            RouteAction::Proxy => RouteActionDto::Proxy,
            RouteAction::Direct => RouteActionDto::Direct,
            RouteAction::Block => RouteActionDto::Block,
        };
        Self { target, action }
    }
}

impl From<RouteRuleDto> for RouteRule {
    fn from(rule: RouteRuleDto) -> Self {
        let target = match rule.target {
            RouteTargetDto::Domain(name) => RouteTarget::Domain(name),
            RouteTargetDto::DomainSuffix(name) => RouteTarget::DomainSuffix(name),
            RouteTargetDto::Cidr(cidr) => RouteTarget::Cidr(cidr),
        };
        let action = match rule.action {
            RouteActionDto::Proxy => RouteAction::Proxy,
            RouteActionDto::Direct => RouteAction::Direct,
            RouteActionDto::Block => RouteAction::Block,
        };
        Self { target, action }
    }
}

impl From<&DnsPolicy> for DnsPolicyDto {
    fn from(dns: &DnsPolicy) -> Self {
        match dns {
            DnsPolicy::Automatic => Self::Automatic,
            DnsPolicy::System => Self::System,
            DnsPolicy::Tcp { endpoint } => Self::Tcp {
                endpoint: *endpoint,
            },
            DnsPolicy::Tls {
                endpoint,
                server_name,
            } => Self::Tls {
                endpoint: *endpoint,
                server_name: server_name.clone(),
            },
            DnsPolicy::Https {
                endpoint,
                server_name,
                path,
            } => Self::Https {
                endpoint: *endpoint,
                server_name: server_name.clone(),
                path: path.clone(),
            },
        }
    }
}

impl From<DnsPolicyDto> for DnsPolicy {
    fn from(dns: DnsPolicyDto) -> Self {
        match dns {
            DnsPolicyDto::Automatic => Self::Automatic,
            DnsPolicyDto::System => Self::System,
            DnsPolicyDto::Tcp { endpoint } => Self::Tcp { endpoint },
            DnsPolicyDto::Tls {
                endpoint,
                server_name,
            } => Self::Tls {
                endpoint,
                server_name,
            },
            DnsPolicyDto::Https {
                endpoint,
                server_name,
                path,
            } => Self::Https {
                endpoint,
                server_name,
                path,
            },
        }
    }
}

impl StoredSubscription {
    fn into_subscription(self) -> Result<Subscription, ClientError> {
        let provider = self
            .provider
            .map(|provider| provider.into_state(&self.url))
            .transpose()
            .map_err(|_| ClientError::StorageCorrupt)?
            .unwrap_or_default();
        Ok(Subscription {
            subscription_uuid: self.subscription_uuid,
            title: self.title,
            auto_title: self.auto_title,
            url: Secret::new(self.url),
            provider,
            unavailable_profiles: self
                .unavailable_profiles
                .into_iter()
                .map(UnavailableProfile::try_from)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<&UnavailableProfile> for StoredUnavailableProfile {
    fn from(profile: &UnavailableProfile) -> Self {
        Self {
            probe: profile.probe_id.as_ref().zip(profile.probe_endpoint()).map(
                |(uuid, endpoint)| {
                    let (host, port) = endpoint.address();
                    StoredUnavailableProbe {
                        uuid: uuid.clone(),
                        host: host.to_owned(),
                        port,
                    }
                },
            ),
            name: profile.name.clone(),
            protocol: profile.protocol.clone(),
            reason: match profile.reason {
                ProfileUnavailableReason::Unsupported => UnavailableReasonDto::Unsupported,
                ProfileUnavailableReason::UnsupportedXhttpUploadMethod => {
                    UnavailableReasonDto::UnsupportedXhttpUploadMethod
                }
                ProfileUnavailableReason::Invalid => UnavailableReasonDto::Invalid,
            },
        }
    }
}

impl TryFrom<StoredUnavailableProfile> for UnavailableProfile {
    type Error = ClientError;

    fn try_from(profile: StoredUnavailableProfile) -> Result<Self, Self::Error> {
        let reason = match profile.reason {
            UnavailableReasonDto::Unsupported => ProfileUnavailableReason::Unsupported,
            UnavailableReasonDto::UnsupportedXhttpUploadMethod => {
                ProfileUnavailableReason::UnsupportedXhttpUploadMethod
            }
            UnavailableReasonDto::Invalid => ProfileUnavailableReason::Invalid,
        };
        let mut row = Self::new(&profile.name, profile.protocol.as_deref(), reason);
        if let Some(probe) = profile.probe {
            let endpoint = vpn_domain::client::TcpProbeEndpoint::new(&probe.host, probe.port)
                .ok_or(ClientError::StorageCorrupt)?;
            row = row.with_probe_endpoint(Some(endpoint));
            row.probe_id = Some(probe.uuid);
        }
        Ok(row)
    }
}

impl StoredProfile {
    fn from_profile(value: &CatalogProfile) -> Self {
        let profile = &value.profile;
        Self {
            profile_uuid: profile.profile_uuid.clone(),
            subscription_uuid: value.subscription_uuid.clone(),
            name: profile.name.clone(),
            source: profile
                .source
                .as_ref()
                .map(|source| source.expose().to_owned()),
            host: profile.host.clone(),
            port: profile.port,
            protocol: ProtocolDto::from_protocol(&profile.protocol),
            transport: match &profile.transport {
                ProxyTransport::Tcp => TransportDto::Tcp,
                ProxyTransport::Udp => TransportDto::Udp,
                ProxyTransport::WebSocket {
                    path,
                    host,
                    early_data,
                } => TransportDto::WebSocket {
                    path: path.clone(),
                    host: host.clone(),
                    early_data: early_data.as_ref().map(|data| WebSocketEarlyDataDto {
                        max_bytes: data.max_bytes,
                        header_name: data.header_name.clone(),
                    }),
                },
                ProxyTransport::Xhttp {
                    path,
                    host,
                    mode,
                    metadata,
                    packet,
                } => TransportDto::Xhttp {
                    path: path.clone(),
                    host: host.clone(),
                    mode: mode.name().to_owned(),
                    metadata: XhttpMetadataDto::from_metadata(metadata),
                    packet: XhttpPacketDto::from_options(packet),
                },
                ProxyTransport::Quic { certificate_sha256 } => TransportDto::Quic {
                    certificate_sha256: certificate_sha256.clone(),
                },
            },
            security: match &profile.security {
                ProxySecurity::None => SecurityDto::None,
                ProxySecurity::Tls {
                    server_name,
                    allow_insecure,
                    alpn,
                    fingerprint,
                } => SecurityDto::Tls {
                    server_name: server_name.clone(),
                    allow_insecure: *allow_insecure,
                    alpn: alpn.as_slice().to_vec(),
                    fingerprint: fingerprint.map(|value| value.name().to_owned()),
                },
                ProxySecurity::Reality {
                    server_name,
                    public_key,
                    short_id,
                    fingerprint,
                } => SecurityDto::Reality {
                    server_name: server_name.clone(),
                    public_key: public_key.clone(),
                    short_id: short_id.clone(),
                    fingerprint: fingerprint.map(|value| value.name().to_owned()),
                },
            },
        }
    }

    fn into_profile(self) -> Result<CatalogProfile, ClientError> {
        let mut restored = CatalogProfile {
            subscription_uuid: self.subscription_uuid,
            profile: ProxyProfile {
                profile_uuid: self.profile_uuid,
                name: self.name,
                source: self.source.map(Secret::new),
                host: self.host,
                port: self.port,
                protocol: self.protocol.into_protocol()?,
                transport: match self.transport {
                    TransportDto::Tcp => ProxyTransport::Tcp,
                    TransportDto::Udp => ProxyTransport::Udp,
                    TransportDto::WebSocket {
                        path,
                        host,
                        early_data,
                    } => ProxyTransport::WebSocket {
                        path,
                        host,
                        early_data: early_data.map(|data| WebSocketEarlyData {
                            max_bytes: data.max_bytes,
                            header_name: data.header_name,
                        }),
                    },
                    TransportDto::Xhttp {
                        path,
                        host,
                        mode,
                        metadata,
                        packet,
                    } => ProxyTransport::Xhttp {
                        path,
                        host,
                        mode: mode.parse().map_err(|_| ClientError::StorageCorrupt)?,
                        metadata: metadata
                            .map(XhttpMetadataDto::into_metadata)
                            .transpose()?
                            .unwrap_or_default(),
                        packet: packet
                            .map(XhttpPacketDto::into_options)
                            .transpose()?
                            .unwrap_or_default(),
                    },
                    TransportDto::Quic { certificate_sha256 } => {
                        ProxyTransport::Quic { certificate_sha256 }
                    }
                },
                security: match self.security {
                    SecurityDto::None => ProxySecurity::None,
                    SecurityDto::Tls {
                        server_name,
                        allow_insecure,
                        alpn,
                        fingerprint,
                    } => ProxySecurity::Tls {
                        server_name,
                        allow_insecure,
                        alpn: AlpnProtocols::new(alpn).map_err(|_| ClientError::StorageCorrupt)?,
                        fingerprint: load_tls_fingerprint(fingerprint.as_deref())?,
                    },
                    SecurityDto::Reality {
                        server_name,
                        public_key,
                        short_id,
                        fingerprint,
                    } => ProxySecurity::Reality {
                        server_name,
                        public_key,
                        short_id,
                        fingerprint: load_tls_fingerprint(fingerprint.as_deref())?,
                    },
                },
            },
        };
        crate::native_profile::set_wireguard_editor_source(&mut restored.profile)
            .map_err(|_| ClientError::StorageCorrupt)?;
        Ok(restored)
    }
}

fn load_tls_fingerprint(value: Option<&str>) -> Result<Option<TlsFingerprint>, ClientError> {
    value
        .map(TlsFingerprint::parse)
        .transpose()
        .map_err(|_| ClientError::StorageCorrupt)
}

impl ProtocolDto {
    fn from_protocol(value: &ProxyProtocol) -> Self {
        let optional =
            |value: &Option<Secret>| value.as_ref().map(|value| value.expose().to_owned());
        match value {
            ProxyProtocol::Graph(graph) => Self::Graph {
                graph: Box::new(GraphDto::from_graph(graph)),
            },
            ProxyProtocol::Native(proxy) => Self::Native {
                settings: proxy.settings.expose().to_owned(),
                dns_servers: proxy.dns_servers.clone(),
            },
            ProxyProtocol::Socks5 { username, password } => Self::Socks5 {
                username: optional(username),
                password: optional(password),
            },
            ProxyProtocol::Http { username, password } => Self::Http {
                username: optional(username),
                password: optional(password),
            },
            ProxyProtocol::Vless {
                user_uuid,
                vision,
                encryption,
            } => Self::Vless {
                user_uuid: user_uuid.expose().to_owned(),
                vision: *vision,
                encryption: optional(encryption),
            },
            ProxyProtocol::Vmess { user_uuid, cipher } => Self::Vmess {
                user_uuid: user_uuid.expose().to_owned(),
                cipher: cipher.clone(),
            },
            ProxyProtocol::Trojan { password } => Self::Trojan {
                password: password.expose().to_owned(),
            },
            ProxyProtocol::Hysteria2 {
                password,
                salamander_password,
                udp_enabled,
            } => Self::Hysteria2 {
                password: password.expose().to_owned(),
                salamander_password: optional(salamander_password),
                udp_enabled: *udp_enabled,
            },
            ProxyProtocol::Shadowsocks { cipher, password } => Self::Shadowsocks {
                cipher: cipher.clone(),
                password: password.expose().to_owned(),
            },
        }
    }

    fn into_protocol(self) -> Result<ProxyProtocol, ClientError> {
        Ok(match self {
            Self::Graph { graph } => ProxyProtocol::Graph(Box::new(graph.into_graph()?)),
            Self::Native {
                settings,
                dns_servers,
            } => ProxyProtocol::Native(crate::native_profile::restore(&settings, dns_servers)?),
            Self::Socks5 { username, password } => ProxyProtocol::Socks5 {
                username: username.map(Secret::new),
                password: password.map(Secret::new),
            },
            Self::Http { username, password } => ProxyProtocol::Http {
                username: username.map(Secret::new),
                password: password.map(Secret::new),
            },
            Self::Vless {
                user_uuid,
                vision,
                encryption,
            } => ProxyProtocol::Vless {
                user_uuid: Secret::new(user_uuid),
                vision,
                encryption: encryption.map(Secret::new),
            },
            Self::Vmess { user_uuid, cipher } => ProxyProtocol::Vmess {
                user_uuid: Secret::new(user_uuid),
                cipher,
            },
            Self::Trojan { password } => ProxyProtocol::Trojan {
                password: Secret::new(password),
            },
            Self::Hysteria2 {
                password,
                salamander_password,
                udp_enabled,
            } => ProxyProtocol::Hysteria2 {
                password: Secret::new(password),
                salamander_password: salamander_password.map(Secret::new),
                udp_enabled,
            },
            Self::Shadowsocks { cipher, password } => ProxyProtocol::Shadowsocks {
                cipher,
                password: Secret::new(password),
            },
        })
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;

    #[test]
    fn native_wireguard_secure_catalog_roundtrip_preserves_protocol_dns_and_source() {
        use vpn_application::client::ClientNormalizerPort;
        let input = b"[Interface]\nPrivateKey = KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=\nAddress = 10.77.0.2/32\nDNS = 10.77.0.1\n[Peer]\nPublicKey = F9firNML9eGdhhroCYjRf66PZi9dgLAFP2zHPNqx0mQ=\nAllowedIPs = 0.0.0.0/0\nEndpoint = 127.0.0.1:51820";
        let profile = crate::client_normalize::SubscriptionNormalizer
            .normalize(input)
            .unwrap()
            .remove(0);
        let row = CatalogProfile {
            subscription_uuid: None,
            profile,
        };
        let serialized =
            Zeroizing::new(serde_json::to_vec(&StoredProfile::from_profile(&row)).unwrap());
        let restored = serde_json::from_slice::<StoredProfile>(&serialized)
            .unwrap()
            .into_profile()
            .unwrap();
        assert_eq!(restored.profile, row.profile);
    }

    #[test]
    fn saved_amnezia_profile_reopens_with_optional_signatures_normalized() {
        use vpn_application::client::ClientNormalizerPort;
        let input = b"[Interface]\nPrivateKey = KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=\nAddress = 10.77.0.2/32\nI1 = <r 5>\n[Peer]\nPublicKey = F9firNML9eGdhhroCYjRf66PZi9dgLAFP2zHPNqx0mQ=\nAllowedIPs = 0.0.0.0/0\nEndpoint = 127.0.0.1:51820";
        let profile = crate::client_normalize::SubscriptionNormalizer
            .normalize(input)
            .unwrap()
            .remove(0);
        let row = CatalogProfile {
            subscription_uuid: None,
            profile,
        };
        let mut stored = StoredProfile::from_profile(&row);
        let ProtocolDto::Native { settings, .. } = &mut stored.protocol else {
            panic!("expected native AmneziaWG");
        };
        let mut old: serde_json::Value = serde_json::from_str(settings).unwrap();
        old["protocol"]["amnezia"]["i2"] = serde_json::Value::from("");
        *settings = old.to_string();
        assert_eq!(stored.into_profile().unwrap().profile, row.profile);
    }

    #[test]
    fn catalog_written_before_provider_fields_loads_without_scheduling_or_fallback() {
        let stored = serde_json::from_str::<StoredCatalog>(
            r#"{
                "version":1,
                "subscriptions":[{"subscription_uuid":"legacy","title":"Legacy","url":"https://example.invalid/sub?key=opaque"}],
                "profiles":[],
                "selected_profile_uuid":null,
                "language":"en",
                "local_proxy_port":0
            }"#,
        )
        .unwrap();
        let catalog = stored.into_catalog().unwrap();
        let subscription = &catalog.subscriptions[0];
        assert_eq!(
            subscription.url.expose(),
            "https://example.invalid/sub?key=opaque"
        );
        assert_eq!(subscription.provider, Default::default());
        assert!(subscription.unavailable_profiles.is_empty());
        let encoded = serde_json::to_value(StoredCatalog::from_catalog(&catalog)).unwrap();
        assert!(
            encoded["subscriptions"][0]
                .get("unavailable_profiles")
                .is_none()
        );
        assert!(catalog.routing.profiles.is_empty());
        assert!(catalog.routing.default_profile_uuid.is_none());
        assert!(catalog.routing.assignments.is_empty());
        assert_eq!(catalog.settings.telemetry, TelemetrySettings::default());
        assert!(encoded.get("telemetry").is_none());
    }

    #[test]
    fn telemetry_settings_round_trip_and_redact_the_bearer() {
        let mut catalog = Catalog::default();
        catalog.settings.telemetry = TelemetrySettings {
            mode: TelemetryMode::Disabled,
            endpoint: Some("https://collector.invalid/custom".to_owned()),
            bearer_token: Some(Secret::new("private-fixture-token")),
        };
        let encoded =
            Zeroizing::new(serde_json::to_vec(&StoredCatalog::from_catalog(&catalog)).unwrap());
        let decoded: StoredCatalog = serde_json::from_slice(&encoded).unwrap();
        let restored = decoded.into_catalog().unwrap();
        assert_eq!(restored.settings.telemetry, catalog.settings.telemetry);
        assert!(!format!("{restored:?}").contains("private-fixture-token"));
    }

    #[test]
    fn catalog_round_trip_preserves_unavailable_profile_summaries() {
        let summaries = vec![
            UnavailableProfile::new(
                "Unsupported transport",
                Some("vless"),
                ProfileUnavailableReason::Unsupported,
            ),
            UnavailableProfile::new("Malformed profile", None, ProfileUnavailableReason::Invalid),
        ];
        let catalog = Catalog {
            subscriptions: vec![Subscription {
                subscription_uuid: "source".to_owned(),
                title: "Source".to_owned(),
                auto_title: false,
                url: Secret::new("https://example.invalid/sub"),
                provider: Default::default(),
                unavailable_profiles: summaries.clone(),
            }],
            ..Catalog::default()
        };
        let wire = serde_json::to_value(StoredCatalog::from_catalog(&catalog)).unwrap();
        assert_eq!(
            wire["subscriptions"][0]["unavailable_profiles"],
            serde_json::json!([
                {"name": "Unsupported transport", "protocol": "vless", "reason": "unsupported"},
                {"name": "Malformed profile", "reason": "invalid"}
            ])
        );
        let restored = serde_json::from_value::<StoredCatalog>(wire)
            .unwrap()
            .into_catalog()
            .unwrap();
        assert_eq!(restored.subscriptions[0].unavailable_profiles, summaries);
    }

    #[test]
    fn routing_catalog_round_trip_preserves_order_dns_and_assignments() {
        let stored: StoredCatalog = serde_json::from_value(serde_json::json!({
            "version": 1,
            "subscriptions":[{"subscription_uuid":"source","title":"Source","url":"https://example.invalid/sub"}],
            "profiles": [],
            "selected_profile_uuid": null,
            "language": "en",
            "local_proxy_port": 0,
            "routing": {
                "default_profile_uuid": "default",
                "profiles": [{
                    "uuid": "default",
                    "revision": 2,
                    "title": "Saved profile",
                    "rules": [
                        {"target": {"type": "domain", "value": "blocked.example"}, "action": "block"},
                        {"target": {"type": "domain_suffix", "value": "example"}, "action": "direct"},
                        {"target": {"type": "cidr", "value": "2001:db8::/32"}, "action": "proxy"}
                    ],
                    "dns": {"type": "https", "endpoint": "[2001:db8::53]:443",
                        "server_name": "dns.example", "path": "/dns-query"}
                }],
                "assignments": {
                    "source": {"profile_uuid": "default", "enabled": true, "ignore_provider": true}
                }
            }
        })).unwrap();
        let before = serde_json::to_value(&stored).unwrap();
        let catalog = stored.into_catalog().unwrap();
        let after = serde_json::to_value(StoredCatalog::from_catalog(&catalog)).unwrap();
        assert_eq!(after["routing"], before["routing"]);
        assert_eq!(catalog.routing.profiles[0].revision, 2);
        assert_eq!(
            catalog.routing.profiles[0].input.rules[0].action,
            RouteAction::Block
        );
    }

    #[test]
    fn stored_tls_profile_preserves_alpn_and_fingerprint() {
        let input = r#"{
            "profile_uuid":"test-profile","name":"TLS fixture","host":"localhost","port":443,
            "protocol":{"type":"http","username":null,"password":null},
            "transport":{"type":"tcp"},
            "security":{"type":"tls","server_name":"localhost","allow_insecure":false}
        }"#;
        let mut profile = serde_json::from_str::<StoredProfile>(input)
            .unwrap()
            .into_profile()
            .unwrap();
        let ProxySecurity::Tls {
            alpn, fingerprint, ..
        } = &mut profile.profile.security
        else {
            panic!("expected the TLS setting");
        };
        assert!(alpn.is_empty());
        assert_eq!(*fingerprint, None);
        *alpn = AlpnProtocols::new(vec!["h2".into(), "http/1.1".into()]).unwrap();
        *fingerprint = Some(TlsFingerprint::Firefox120);
        let bytes = serde_json::to_vec(&StoredProfile::from_profile(&profile)).unwrap();
        assert_eq!(
            serde_json::from_slice::<StoredProfile>(&bytes)
                .unwrap()
                .into_profile(),
            Ok(profile)
        );
    }

    #[test]
    fn websocket_profiles_read_legacy_defaults_and_preserve_early_data() {
        let input = r#"{
            "profile_uuid":"test-profile","name":"WebSocket fixture","host":"localhost","port":443,
            "protocol":{"type":"http","username":null,"password":null},
            "transport":{"type":"web_socket","path":"/ws","host":"localhost"},
            "security":{"type":"none"}
        }"#;
        let mut profile = serde_json::from_str::<StoredProfile>(input)
            .unwrap()
            .into_profile()
            .unwrap();
        let ProxyTransport::WebSocket { early_data, .. } = &mut profile.profile.transport else {
            panic!("expected stored WebSocket profile");
        };
        assert!(early_data.is_none());
        for header in [None, Some("sec-websocket-protocol".to_owned())] {
            let ProxyTransport::WebSocket { early_data, .. } = &mut profile.profile.transport
            else {
                unreachable!();
            };
            *early_data = Some(WebSocketEarlyData {
                max_bytes: NonZeroU16::new(2560).unwrap(),
                header_name: header,
            });
            let encoded = serde_json::to_vec(&StoredProfile::from_profile(&profile)).unwrap();
            let restored = serde_json::from_slice::<StoredProfile>(&encoded)
                .unwrap()
                .into_profile()
                .unwrap();
            assert_eq!(restored, profile);
        }
    }

    #[test]
    fn vless_encryption_survives_catalog_serialization() {
        let setting = Secret::new("synthetic-native-vless-setting");
        let protocol = ProxyProtocol::Vless {
            user_uuid: Secret::new("fixture"),
            vision: true,
            encryption: Some(setting),
        };
        let bytes =
            Zeroizing::new(serde_json::to_vec(&ProtocolDto::from_protocol(&protocol)).unwrap());
        let restored = serde_json::from_slice::<ProtocolDto>(&bytes).unwrap();
        assert_eq!(restored.into_protocol(), Ok(protocol));
    }

    #[test]
    fn hysteria2_profile_survives_catalog_serialization() {
        let wire = serde_json::json!({
            "profile_uuid":"fixture","subscription_uuid":null,"name":"Hysteria2","host":"localhost","port":443,
            "protocol":{"type":"hysteria2","password":"fixture","salamander_password":"fixture-obfs","udp_enabled":false},
            "transport":{"type":"quic","certificate_sha256":"a1".repeat(32)},
            "security":{"type":"tls","server_name":"localhost","allow_insecure":false,"alpn":["h3"]}
        });
        let profile = serde_json::from_value::<StoredProfile>(wire.clone())
            .unwrap()
            .into_profile()
            .unwrap();
        assert_eq!(
            serde_json::to_value(StoredProfile::from_profile(&profile)).unwrap(),
            wire
        );
    }

    #[test]
    fn xhttp_profile_survives_catalog_serialization() {
        let wire = r#"{
            "profile_uuid":"fixture","subscription_uuid":null,"name":"XHTTP","host":"localhost","port":443,
            "protocol":{"type":"vless","user_uuid":"fixture","vision":false},
            "transport":{"type":"xhttp","path":"/proxy?cache=stable","host":"edge.example.test","mode":"stream-up"},
            "security":{"type":"tls","server_name":"localhost","allow_insecure":false,"alpn":["h3"]}
        }"#;
        let original: serde_json::Value = serde_json::from_str(wire).unwrap();
        let mut named = original.clone();
        named["transport"]["metadata"] = serde_json::json!({
            "session_id_placement":"query","session_id_key":"visit",
            "seq_placement":"cookie","seq_key":"packet"
        });
        let mut tuned = named.clone();
        tuned["transport"]["packet"] = serde_json::json!({
            "max_post_bytes":[128000,256000], "min_post_interval_ms":[5,15]
        });
        for stored in [original, named, tuned] {
            let profile = serde_json::from_value::<StoredProfile>(stored.clone())
                .unwrap()
                .into_profile()
                .unwrap();
            let serialized = serde_json::to_value(StoredProfile::from_profile(&profile)).unwrap();
            assert_eq!(serialized, stored);
        }
    }
}
