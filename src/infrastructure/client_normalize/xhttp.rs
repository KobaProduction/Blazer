use serde_json::Value;
use vpn_domain::client::{XhttpMetadata, XhttpMode, XhttpPacketOptions, XhttpPacketRange};

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
const UPLINK_METHOD: &[&str] = &["uplink_http_method", "uplinkHTTPMethod"];
const ALIASES: [&[&str]; 7] = [
    SESSION_PLACEMENT,
    SESSION_KEY,
    SEQ_PLACEMENT,
    SEQ_KEY,
    MAX_POST_BYTES,
    MIN_POST_INTERVAL_MS,
    UPLINK_METHOD,
];

#[derive(Default)]
pub(super) struct Options {
    metadata: XhttpMetadata,
    packet: XhttpPacketOptions,
}

pub(super) enum ValueRef<'a> {
    Text(&'a str),
    Json(&'a Value),
}

impl<'a> ValueRef<'a> {
    fn text(self) -> Result<&'a str, ClientError> {
        match self {
            Self::Text(value) => Ok(value),
            Self::Json(value) => value.as_str().ok_or(ClientError::InvalidSubscription),
        }
    }

    fn range(self) -> Result<Option<XhttpPacketRange>, ClientError> {
        let (min, max) = match self {
            Self::Json(Value::Number(value)) => {
                let value = value
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or(ClientError::InvalidSubscription)?;
                (value, value)
            }
            other => {
                let text = other.text()?;
                let (min, max) = text.split_once('-').unwrap_or((text, text));
                (parse_bound(min)?, parse_bound(max)?)
            }
        };
        if min == 0 && max == 0 {
            return Ok(None);
        }
        if max > i32::MAX as u32 {
            return Err(ClientError::UnsupportedFeature);
        }
        XhttpPacketRange::new(min, max)
            .map(Some)
            .map_err(|_| ClientError::InvalidSubscription)
    }
}

fn parse_bound(value: &str) -> Result<u32, ClientError> {
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ClientError::InvalidSubscription);
    }
    value.parse().map_err(|_| ClientError::InvalidSubscription)
}

pub(super) fn is_xhttp_option(key: &str) -> bool {
    ALIASES.iter().any(|aliases| aliases.contains(&key))
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
        if ALIASES
            .iter()
            .copied()
            .flatten()
            .any(|key| get(key).is_some())
        {
            return Err(ClientError::UnsupportedFeature);
        }
        fields_with_options(extra, &[])?;
        return parse_options(|key| extra.get(key).map(ValueRef::Json));
    }
    parse_options(get)
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
    if text(UPLINK_METHOD)?.is_some_and(is_unsupported_upload_method) {
        return Err(ClientError::UnsupportedXhttpUploadMethod);
    }
    let range = |aliases: &[&str]| {
        Ok::<_, ClientError>(
            unique_alias(&get, aliases)?
                .map(ValueRef::range)
                .transpose()?
                .flatten(),
        )
    };
    Ok(Options {
        metadata,
        packet: XhttpPacketOptions {
            max_post_bytes: range(MAX_POST_BYTES)?,
            min_post_interval_ms: range(MIN_POST_INTERVAL_MS)?,
        },
    })
}

fn is_unsupported_upload_method(method: &str) -> bool {
    !method.is_empty() && !method.eq_ignore_ascii_case("POST")
}

pub(super) fn unsupported_upload_method(value: &Value) -> bool {
    let effective = value.get("extra").unwrap_or(value);
    unique_alias(|key| effective.get(key), UPLINK_METHOD)
        .ok()
        .flatten()
        .and_then(Value::as_str)
        .is_some_and(is_unsupported_upload_method)
}

fn unique_alias<T>(
    get: impl Fn(&str) -> Option<T>,
    aliases: &[&str],
) -> Result<Option<T>, ClientError> {
    let mut found = None;
    for alias in aliases {
        if let Some(value) = get(alias)
            && found.replace(value).is_some()
        {
            return Err(ClientError::InvalidSubscription);
        }
    }
    Ok(found)
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
