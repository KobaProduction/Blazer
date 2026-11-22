use std::net::SocketAddr;

use sysproxy::Sysproxy;
use vpn_domain::client::ClientError;
use windows_registry::{CURRENT_USER, Key, Value};
use zeroize::Zeroizing;

const INTERNET: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
const CONNECTIONS: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Internet Settings\Connections";
const JOURNAL: &str = r"Software\KobaProduction\Blazer\SystemProxy";
const VALUES: [(&str, &str); 7] = [
    (INTERNET, "ProxyEnable"),
    (INTERNET, "ProxyServer"),
    (INTERNET, "ProxyOverride"),
    (INTERNET, "AutoConfigURL"),
    (INTERNET, "AutoDetect"),
    (CONNECTIONS, "DefaultConnectionSettings"),
    (CONNECTIONS, "SavedLegacySettings"),
];

#[derive(Clone)]
struct Snapshot([Option<Value>; 7]);
type SerializedSnapshot = [Option<(u32, Vec<u8>)>; 7];

pub(super) struct ProxyLease {
    original: Snapshot,
    applied: Option<Snapshot>,
    restored: bool,
}

impl ProxyLease {
    pub(super) fn apply(address: SocketAddr) -> Result<Self, ClientError> {
        check_policy()?;
        recover()?;
        let original = Snapshot::read()?;
        original.notification()?;
        let mut lease = Self {
            original,
            applied: None,
            restored: false,
        };
        lease.save_journal(address)?;
        if let Err(error) = enable(address) {
            lease.restore()?;
            return Err(error);
        }
        lease.applied = Some(Snapshot::read()?);
        lease.save_journal(address)?;
        Ok(lease)
    }

    pub(super) fn restore(&mut self) -> Result<(), ClientError> {
        if self.restored {
            return Ok(());
        }
        self.restore_inner()
            .map_err(|_| ClientError::SystemProxyRestoreFailed)?;
        clear_journal()?;
        self.restored = true;
        Ok(())
    }

    fn save_journal(&self, address: SocketAddr) -> Result<(), ClientError> {
        let journal = Journal {
            original: self.original.serialized(),
            applied: self.applied.as_ref().map(Snapshot::serialized),
            address,
        };
        let plain = Zeroizing::new(
            serde_json::to_vec(&journal).map_err(|_| ClientError::SystemProxyUnavailable)?,
        );
        let sealed = windows_dpapi::encrypt_data(
            &plain,
            windows_dpapi::Scope::User,
            Some(b"Blazer.SystemProxy"),
        )
        .map_err(|_| ClientError::SystemProxyUnavailable)?;
        CURRENT_USER
            .create(JOURNAL)
            .and_then(|key| key.set_value("Recovery", &Value::from(sealed.as_slice())))
            .map_err(|_| ClientError::SystemProxyUnavailable)
    }

    fn restore_inner(&self) -> Result<(), ClientError> {
        let current = Snapshot::read()?;
        if let Some(applied) = &self.applied {
            // The proxy endpoint is the ownership token. A user's explicit
            // replacement takes precedence over our previous configuration.
            if !applied.owns(&current) {
                return Ok(());
            }
        }
        let desired = match &self.applied {
            Some(applied) => self.original.merge_unchanged(applied, current),
            None => self.original.clone(),
        };
        // Notify WinINet with the restored semantic values. Reapply the raw
        // snapshot afterwards to retain original value types and absent values.
        // Connection blobs include counters which WinINet may change on refresh.
        let notification = desired.notification()?;
        desired.write()?;
        let notified = notification
            .set_system_proxy()
            .map_err(|_| ClientError::SystemProxyRestoreFailed);
        let restored = desired.write();
        notified.and(restored)
    }
}

impl Drop for ProxyLease {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

impl Snapshot {
    fn owns(&self, current: &Self) -> bool {
        self.0[..2] == current.0[..2]
    }

    fn read() -> Result<Self, ClientError> {
        let mut values = std::array::from_fn(|_| None);
        for (index, (path, name)) in VALUES.iter().enumerate() {
            values[index] = read_value(path, name)?;
        }
        Ok(Self(values))
    }

    fn write(&self) -> Result<(), ClientError> {
        for ((path, name), value) in VALUES.iter().zip(&self.0) {
            let key = CURRENT_USER
                .create(path)
                .map_err(|_| ClientError::SystemProxyUnavailable)?;
            write_value(&key, name, value.as_ref())?;
        }
        Ok(())
    }

    fn merge_unchanged(&self, applied: &Self, mut current: Self) -> Self {
        for (index, value) in current.0.iter_mut().enumerate() {
            if *value == applied.0[index] {
                *value = self.0[index].clone();
            }
        }
        // WinINet changes counters in these blobs without changing ownership.
        // Never retain a blob which may still name the terminated listener.
        current.0[5..].clone_from_slice(&self.0[5..]);
        current
    }

    fn serialized(&self) -> SerializedSnapshot {
        std::array::from_fn(|index| {
            self.0[index]
                .as_ref()
                .map(|value| (value.ty().into(), value.to_vec()))
        })
    }

    fn from_serialized(values: SerializedSnapshot) -> Self {
        Self(values.map(|value| {
            value.map(|(ty, bytes)| {
                let mut value = Value::from(bytes.as_slice());
                value.set_ty(ty.into());
                value
            })
        }))
    }

    fn notification(&self) -> Result<Sysproxy, ClientError> {
        let enabled = self.0[0]
            .clone()
            .map(u32::try_from)
            .transpose()
            .map_err(|_| ClientError::SystemProxyUnavailable)?
            .unwrap_or(0)
            != 0;
        let server = string_value(&self.0[1])?;
        let (host, port) = notification_server(&server, enabled)?;
        Ok(Sysproxy {
            enable: enabled,
            host,
            port,
            bypass: string_value(&self.0[2])?,
        })
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Journal {
    original: SerializedSnapshot,
    applied: Option<SerializedSnapshot>,
    address: SocketAddr,
}

pub(super) fn recover() -> Result<(), ClientError> {
    let Some(sealed) = read_value(JOURNAL, "Recovery")? else {
        return Ok(());
    };
    if sealed.len() > 128 * 1024 {
        return Err(ClientError::SystemProxyRestoreFailed);
    }
    let plain = Zeroizing::new(
        windows_dpapi::decrypt_data(
            &sealed,
            windows_dpapi::Scope::User,
            Some(b"Blazer.SystemProxy"),
        )
        .map_err(|_| ClientError::SystemProxyRestoreFailed)?,
    );
    let journal: Journal =
        serde_json::from_slice(&plain).map_err(|_| ClientError::SystemProxyRestoreFailed)?;
    let current = Snapshot::read()?;
    let original = Snapshot::from_serialized(journal.original);
    let our_server = string_value(&current.0[1])? == journal.address.to_string();
    // The setter writes ProxyEnable before ProxyServer. A pending journal
    // also owns this intermediate state if its previous endpoint is intact.
    let unchanged_server = current.0[1] == original.0[1];
    if our_server || (journal.applied.is_none() && unchanged_server) {
        let mut lease = ProxyLease {
            original,
            applied: journal.applied.map(Snapshot::from_serialized),
            restored: false,
        };
        lease.restore()?;
    }
    clear_journal()
}

fn clear_journal() -> Result<(), ClientError> {
    let key = CURRENT_USER
        .create(JOURNAL)
        .map_err(|_| ClientError::SystemProxyRestoreFailed)?;
    write_value(&key, "Recovery", None).map_err(|_| ClientError::SystemProxyRestoreFailed)
}

fn notification_server(server: &str, enabled: bool) -> Result<(String, u16), ClientError> {
    if server.is_empty() && !enabled {
        return Ok(("127.0.0.1".into(), 9));
    }
    let (host, port) = server
        .rsplit_once(':')
        .ok_or(ClientError::SystemProxyUnavailable)?;
    let port = port
        .parse()
        .map_err(|_| ClientError::SystemProxyUnavailable)?;
    Ok((host.to_owned(), port))
}

fn string_value(value: &Option<Value>) -> Result<String, ClientError> {
    value
        .clone()
        .map(String::try_from)
        .transpose()
        .map_err(|_| ClientError::SystemProxyUnavailable)
        .map(Option::unwrap_or_default)
}

fn read_value(path: &str, name: &str) -> Result<Option<Value>, ClientError> {
    let key = match CURRENT_USER.open(path) {
        Ok(key) => key,
        Err(error) if error.code().0 as u32 == 0x80070002 => return Ok(None),
        Err(_) => return Err(ClientError::SystemProxyUnavailable),
    };
    match key.get_value(name) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.code().0 as u32 == 0x80070002 => Ok(None),
        Err(_) => Err(ClientError::SystemProxyUnavailable),
    }
}

fn write_value(key: &Key, name: &str, value: Option<&Value>) -> Result<(), ClientError> {
    match value {
        Some(value) => key
            .set_value(name, value)
            .map_err(|_| ClientError::SystemProxyUnavailable),
        None => match key.remove_value(name) {
            Ok(()) => Ok(()),
            Err(error) if error.code().0 as u32 == 0x80070002 => Ok(()),
            Err(_) => Err(ClientError::SystemProxyUnavailable),
        },
    }
}

fn check_policy() -> Result<(), ClientError> {
    use windows_registry::LOCAL_MACHINE;
    let path = r"Software\Policies\Microsoft\Windows\CurrentVersion\Internet Settings";
    if LOCAL_MACHINE
        .open(path)
        .ok()
        .and_then(|key| key.get_u32("ProxySettingsPerUser").ok())
        == Some(0)
    {
        return Err(ClientError::SystemProxyUnavailable);
    }
    Ok(())
}

fn enable(address: SocketAddr) -> Result<(), ClientError> {
    let key = CURRENT_USER
        .create(INTERNET)
        .map_err(|_| ClientError::SystemProxyUnavailable)?;
    write_value(&key, "AutoConfigURL", None)?;
    key.set_u32("AutoDetect", 0)
        .map_err(|_| ClientError::SystemProxyUnavailable)?;
    configure_connection(address)?;
    Sysproxy {
        enable: true,
        host: address.ip().to_string(),
        port: address.port(),
        bypass: "<local>;localhost;127.*;[::1]".into(),
    }
    .set_system_proxy()
    .map_err(|_| ClientError::SystemProxyUnavailable)
}

fn configure_connection(address: SocketAddr) -> Result<(), ClientError> {
    let Some(value) = read_value(CONNECTIONS, "DefaultConnectionSettings")? else {
        return Ok(());
    };
    let bytes = connection_bytes(&value, &address.to_string())?;
    let key = CURRENT_USER
        .create(CONNECTIONS)
        .map_err(|_| ClientError::SystemProxyUnavailable)?;
    key.set_bytes("DefaultConnectionSettings", value.ty(), &bytes)
        .map_err(|_| ClientError::SystemProxyUnavailable)
}

fn connection_bytes(original: &[u8], server: &str) -> Result<Vec<u8>, ClientError> {
    let header = original
        .get(..12)
        .ok_or(ClientError::SystemProxyUnavailable)?;
    let mut position = 12;
    for _ in 0..3 {
        let bytes = original
            .get(position..position + 4)
            .ok_or(ClientError::SystemProxyUnavailable)?;
        let length = u32::from_le_bytes(
            bytes
                .try_into()
                .map_err(|_| ClientError::SystemProxyUnavailable)?,
        ) as usize;
        position = position
            .checked_add(4 + length)
            .ok_or(ClientError::SystemProxyUnavailable)?;
    }
    let tail = original
        .get(position..)
        .ok_or(ClientError::SystemProxyUnavailable)?;
    let mut bytes = header.to_vec();
    bytes[8..12].copy_from_slice(&3u32.to_le_bytes());
    for value in [server, "<local>;localhost;127.*;[::1]", ""] {
        bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
        bytes.extend_from_slice(value.as_bytes());
    }
    bytes.extend_from_slice(tail);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_preserves_protocol_specific_and_ipv6_servers() {
        for server in [
            "http=proxy.example:3128;https=secure.example:8443",
            "[::1]:1080",
            "proxy.example:8080",
        ] {
            let (host, port) = notification_server(server, true).unwrap();
            assert_eq!(format!("{host}:{port}"), server);
        }
    }

    #[test]
    fn malformed_original_proxy_is_rejected_without_mutation() {
        for server in ["invalid", "proxy:abc", "proxy:99999", ""] {
            assert_eq!(
                notification_server(server, true),
                Err(ClientError::SystemProxyUnavailable)
            );
        }
    }

    #[test]
    fn replacing_the_proxy_releases_ownership_without_disabling_it() {
        let original = Snapshot(std::array::from_fn(|_| None));
        let mut applied = original.clone();
        applied.0[0] = Some(Value::from(1u32));
        applied.0[1] = Some(Value::from("127.0.0.1:1080"));
        let mut changed = applied.clone();
        changed.0[1] = Some(Value::from("new.example:8080"));
        assert!(applied.owns(&applied));
        assert!(!applied.owns(&changed));
        changed.0[1] = applied.0[1].clone();
        changed.0[0] = Some(Value::from(0u32));
        assert!(!applied.owns(&changed));
    }

    #[test]
    fn recovery_preserves_raw_types_absence_and_connection_blobs() {
        let mut snapshot = Snapshot(std::array::from_fn(|_| None));
        snapshot.0[0] = Some(Value::from(0u32));
        snapshot.0[1] = Some(Value::from("http=host:3128;https=host:8443"));
        snapshot.0[5] = Some(Value::from([0u8, 255, 1, 2].as_slice()));
        let recovered = Snapshot::from_serialized(snapshot.serialized());
        assert_eq!(recovered.0, snapshot.0);
    }
}
