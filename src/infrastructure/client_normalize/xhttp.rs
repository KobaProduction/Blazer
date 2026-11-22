use serde_json::Value;
use vpn_domain::client::{XhttpMetadata, XhttpMode, XhttpPacketOptions, XhttpPacketRange};

use crate::xhttp_wire::{self, unique_alias};

pub(super) use crate::xhttp_wire::Input as ValueRef;

use super::json::{check_empty, optional_str};
use super::{ClientError, ProxyTransport, http_authority};

const SESSION_PLACEMENT: &[&str] = &[
    "session_id_placement",
    "session_placement",
    "sessionIDPlacement",
    "sessionPlacement",
];
const SESSION_KEY: &[&str] = &[
    "session_id_key",
    "session_key",
    "sessionIDKey",
    "sessionKey",
];
const SEQ_PLACEMENT: &[&str] = &["seq_placement", "seqPlacement"];
const SEQ_KEY: &[&str] = &["seq_key", "seqKey"];
const MAX_POST_BYTES: &[&str] = &["sc_max_each_post_bytes", "scMaxEachPostBytes"];
const MIN_POST_INTERVAL_MS: &[&str] = &["sc_min_posts_interval_ms", "scMinPostsIntervalMs"];
const ALIASES: [&[&str]; 6] = [
    SESSION_PLACEMENT,
    SESSION_KEY,
    SEQ_PLACEMENT,
    SEQ_KEY,
    MAX_POST_BYTES,
    MIN_POST_INTERVAL_MS,
];

#[derive(Default)]
pub(super) struct Options {
    metadata: XhttpMetadata,
    packet: XhttpPacketOptions,
}

pub(super) fn is_xhttp_option(key: &str) -> bool {
    xhttp_wire::recognizes(key) || ALIASES.iter().any(|aliases| aliases.contains(&key))
}

pub(super) fn fields_with_options(value: &Value, allowed: &[&str]) -> Result<(), ClientError> {
    let object = value.as_object().ok_or(ClientError::InvalidSubscription)?;
    if object
        .keys()
        .any(|key| !allowed.contains(&key.as_str()) && !is_xhttp_option(key))
    {
        return Err(ClientError::UnsupportedFeature);
    }
    Ok(())
}

pub(super) fn json_transport(
    value: &Value,
    variant_field: &str,
) -> Result<ProxyTransport, ClientError> {
    fields_with_options(value, &["path", "host", "mode", "headers", variant_field])?;
    check_empty(value, "headers", |headers| {
        headers.as_object().is_some_and(serde_json::Map::is_empty)
    })?;
    transport(
        optional_str(value, "path")?.unwrap_or("/"),
        optional_str(value, "host")?,
        optional_str(value, "mode")?,
        json_options(value)?,
    )
}

pub(super) fn json_options(value: &Value) -> Result<Options, ClientError> {
    options(|key| value.get(key).map(ValueRef::Json), value.get("extra"))
}

pub(super) fn options<'a>(
    get: impl Fn(&str) -> Option<ValueRef<'a>>,
    extra: Option<&Value>,
) -> Result<Options, ClientError> {
    if let Some(extra) = extra {
        // Xray takes transport options from extra, apart from the outer host, path and mode.
        // Reject mixed input instead of merging options that Xray would discard.
        if xhttp_wire::has_options(&get)
            || ALIASES
                .iter()
                .flat_map(|aliases| aliases.iter())
                .any(|key| get(key).is_some())
        {
            return Err(ClientError::UnsupportedFeature);
        }
        fields_with_options(extra, &[])?;
        return parse_options(|key| extra.get(key).map(ValueRef::Json));
    }
    parse_options(get)
}

fn packet_range(value: ValueRef<'_>) -> Result<Option<XhttpPacketRange>, ClientError> {
    match value.bounds()? {
        (0, 0) => Ok(None),
        (min, max) => XhttpPacketRange::new(min, max)
            .map(Some)
            .map_err(|_| ClientError::InvalidSubscription),
    }
}

fn parse_options<'a>(get: impl Fn(&str) -> Option<ValueRef<'a>>) -> Result<Options, ClientError> {
    let text = |aliases: &[&str]| unique_alias(&get, aliases)?.map(ValueRef::text).transpose();
    let placement = |aliases: &[&str]| text(aliases)?.unwrap_or_default().parse();
    let key = |aliases: &[&str]| {
        Ok::<_, ClientError>(
            text(aliases)?
                .filter(|key| !key.is_empty())
                .map(str::to_owned),
        )
    };
    let metadata = XhttpMetadata {
        session_id_placement: placement(SESSION_PLACEMENT)?,
        session_id_key: key(SESSION_KEY)?,
        seq_placement: placement(SEQ_PLACEMENT)?,
        seq_key: key(SEQ_KEY)?,
    };
    metadata.validate().map_err(|error| match error {
        ClientError::InvalidInput => ClientError::InvalidSubscription,
        _ => error,
    })?;
    let range = |aliases: &[&str]| {
        Ok::<_, ClientError>(
            unique_alias(&get, aliases)?
                .map(packet_range)
                .transpose()?
                .flatten(),
        )
    };
    Ok(Options {
        metadata,
        packet: XhttpPacketOptions {
            max_post_bytes: range(MAX_POST_BYTES)?,
            min_post_interval_ms: range(MIN_POST_INTERVAL_MS)?,
            wire: xhttp_wire::parse(get)?,
        },
    })
}

pub(super) fn transport(
    path: &str,
    host: Option<&str>,
    mode: Option<&str>,
    options: Options,
) -> Result<ProxyTransport, ClientError> {
    if path.len() > 4096
        || path.chars().any(|ch| ch.is_whitespace() || ch.is_control())
        || path.contains('#')
    {
        return Err(ClientError::InvalidSubscription);
    }
    let mode = mode
        .filter(|value| !value.is_empty())
        .unwrap_or("auto")
        .parse::<XhttpMode>()?;
    options.packet.validate_mode(mode)?;
    let host = http_authority(host)?;
    if host.as_ref().is_some_and(|value| !value.is_ascii()) {
        return Err(ClientError::InvalidSubscription);
    }
    Ok(ProxyTransport::Xhttp {
        path: path.to_owned(),
        host,
        mode,
        metadata: options.metadata,
        packet: options.packet,
    })
}
