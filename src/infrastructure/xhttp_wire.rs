use serde_json::{Map, Value};
use vpn_domain::client::{ClientError, XhttpWireField, XhttpWireOptions, XhttpWireValue};

use XhttpWireField::*;
use XhttpWireValue::{Flag, Range, Seconds, Text};

#[derive(Clone, Copy)]
pub(crate) enum Input<'a> {
    Text(&'a str),
    Json(&'a Value),
}

impl<'a> Input<'a> {
    pub(crate) fn text(self) -> Result<&'a str, ClientError> {
        match self {
            Self::Text(value) => Ok(value),
            Self::Json(value) => value.as_str().ok_or(ClientError::InvalidSubscription),
        }
    }

    pub(crate) fn bounds(self) -> Result<(u32, u32), ClientError> {
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
        if max > i32::MAX as u32 {
            return Err(ClientError::UnsupportedFeature);
        }
        if min > max {
            return Err(ClientError::InvalidSubscription);
        }
        Ok((min, max))
    }
}

fn parse_bound(value: &str) -> Result<u32, ClientError> {
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ClientError::InvalidSubscription);
    }
    value.parse().map_err(|_| ClientError::InvalidSubscription)
}

type Parser = fn(Input<'_>) -> Result<Option<XhttpWireValue>, ClientError>;
type Field = (XhttpWireField, &'static str, &'static str, Parser);

const FIELDS: &[Field] = &[
    (
        UplinkMethod,
        "uplink_http_method",
        "uplinkHTTPMethod",
        method,
    ),
    (
        UplinkPlacement,
        "uplink_data_placement",
        "uplinkDataPlacement",
        uplink_placement,
    ),
    (UplinkKey, "uplink_data_key", "uplinkDataKey", header),
    (
        UplinkChunkSize,
        "uplink_chunk_size",
        "uplinkChunkSize",
        range,
    ),
    (SessionTable, "session_id_table", "sessionIDTable", text),
    (SessionLength, "session_id_length", "sessionIDLength", range),
    (PaddingBytes, "x_padding_bytes", "xPaddingBytes", range),
    (
        PaddingObfuscation,
        "x_padding_obfs_mode",
        "xPaddingObfsMode",
        flag,
    ),
    (PaddingKey, "x_padding_key", "xPaddingKey", text),
    (PaddingHeader, "x_padding_header", "xPaddingHeader", header),
    (
        PaddingPlacement,
        "x_padding_placement",
        "xPaddingPlacement",
        padding_placement,
    ),
    (
        PaddingMethod,
        "x_padding_method",
        "xPaddingMethod",
        padding_method,
    ),
    (Xmux, "xmux", "xmux", xmux),
];

const XMUX_FIELDS: &[Field] = &[
    (MaxConnections, "max_connections", "maxConnections", range),
    (MaxConcurrency, "max_concurrency", "maxConcurrency", range),
    (
        MaxConnectionReuses,
        "c_max_reuse_times",
        "cMaxReuseTimes",
        range,
    ),
    (
        MaxHttpRequests,
        "h_max_request_times",
        "hMaxRequestTimes",
        range,
    ),
    (
        MaxHttpReuseSeconds,
        "h_max_reusable_secs",
        "hMaxReusableSecs",
        range,
    ),
    (
        HttpKeepAliveSeconds,
        "h_keep_alive_period",
        "hKeepAlivePeriod",
        seconds,
    ),
];

pub(crate) fn recognizes(key: &str) -> bool {
    recognizes_field(FIELDS, key)
}

pub(crate) fn has_options<'a>(get: impl Fn(&str) -> Option<Input<'a>>) -> bool {
    FIELDS
        .iter()
        .any(|&(_, name, alias, _)| get(name).is_some() || get(alias).is_some())
}

fn recognizes_field(fields: &[Field], key: &str) -> bool {
    fields
        .iter()
        .any(|&(_, name, alias, _)| key == name || key == alias)
}

pub(crate) fn parse<'a>(
    get: impl Fn(&str) -> Option<Input<'a>>,
) -> Result<XhttpWireOptions, ClientError> {
    parse_fields(FIELDS, get)
}

pub(crate) fn restore(values: &Map<String, Value>) -> Result<XhttpWireOptions, ClientError> {
    parse_object(FIELDS, values).map_err(|_| ClientError::StorageCorrupt)
}

fn parse_object(
    fields: &[Field],
    object: &Map<String, Value>,
) -> Result<XhttpWireOptions, ClientError> {
    if object.keys().any(|key| !recognizes_field(fields, key)) {
        return Err(ClientError::UnsupportedFeature);
    }
    parse_fields(fields, |key| object.get(key).map(Input::Json))
}

fn parse_fields<'a>(
    fields: &[Field],
    get: impl Fn(&str) -> Option<Input<'a>>,
) -> Result<XhttpWireOptions, ClientError> {
    let mut options = XhttpWireOptions::new();
    for &(field, name, alias, parse) in fields {
        let aliases = if name == alias {
            &[name][..]
        } else {
            &[name, alias][..]
        };
        if let Some(value) = unique_alias(&get, aliases)?
            && let Some(value) = parse(value)?
        {
            options.insert(field, value);
        }
    }
    Ok(options)
}

pub(crate) fn unique_alias<T>(
    get: impl Fn(&str) -> Option<T>,
    aliases: &[&str],
) -> Result<Option<T>, ClientError> {
    let mut found = aliases.iter().filter_map(|alias| get(alias));
    let first = found.next();
    if found.next().is_some() {
        return Err(ClientError::InvalidSubscription);
    }
    Ok(first)
}

pub(crate) fn render(options: &XhttpWireOptions) -> Map<String, Value> {
    render_fields(FIELDS, options)
}

fn render_fields(fields: &[Field], options: &XhttpWireOptions) -> Map<String, Value> {
    fields
        .iter()
        .filter_map(|&(field, name, _, _)| {
            options
                .get(&field)
                .map(|value| (name.to_owned(), render_value(value)))
        })
        .collect()
}

fn render_value(value: &XhttpWireValue) -> Value {
    match value {
        Text(value) => Value::from(value.clone()),
        Range(min, max) => render_range(*min, *max),
        Flag(value) => Value::from(*value),
        Seconds(value) => Value::from(*value),
        XhttpWireValue::Xmux(options) => Value::Object(render_fields(XMUX_FIELDS, options)),
    }
}

pub(crate) fn render_range(min: u32, max: u32) -> Value {
    if min == max {
        Value::from(min)
    } else {
        Value::from(format!("{min}-{max}"))
    }
}

fn method(input: Input<'_>) -> Result<Option<XhttpWireValue>, ClientError> {
    let value = input.text()?.to_ascii_uppercase();
    if value.is_empty() || value == "POST" {
        return Ok(None);
    }
    reqwest::Method::from_bytes(value.as_bytes()).map_err(|_| ClientError::InvalidSubscription)?;
    Ok(Some(Text(value)))
}

fn header(input: Input<'_>) -> Result<Option<XhttpWireValue>, ClientError> {
    let value = input.text()?;
    if !value.is_empty() {
        reqwest::header::HeaderName::from_bytes(value.as_bytes())
            .map_err(|_| ClientError::InvalidSubscription)?;
    }
    text(input)
}

fn text(input: Input<'_>) -> Result<Option<XhttpWireValue>, ClientError> {
    let value = input.text()?;
    if value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(ClientError::InvalidSubscription);
    }
    Ok((!value.is_empty()).then(|| Text(value.to_owned())))
}

fn uplink_placement(input: Input<'_>) -> Result<Option<XhttpWireValue>, ClientError> {
    match input.text()? {
        "auto" | "" => Ok(None),
        "body" | "header" | "cookie" => text(input),
        _ => Err(ClientError::UnsupportedFeature),
    }
}

fn padding_placement(input: Input<'_>) -> Result<Option<XhttpWireValue>, ClientError> {
    match input.text()? {
        "queryInHeader" | "query-in-header" | "" => Ok(None),
        "query" | "header" | "cookie" => text(input),
        _ => Err(ClientError::UnsupportedFeature),
    }
}

fn padding_method(input: Input<'_>) -> Result<Option<XhttpWireValue>, ClientError> {
    match input.text()? {
        "repeat-x" | "" => Ok(None),
        "tokenish" => text(input),
        _ => Err(ClientError::UnsupportedFeature),
    }
}

fn flag(input: Input<'_>) -> Result<Option<XhttpWireValue>, ClientError> {
    let value = match input {
        Input::Json(Value::Bool(value)) => *value,
        _ => input
            .text()?
            .parse()
            .map_err(|_| ClientError::InvalidSubscription)?,
    };
    Ok(Some(Flag(value)))
}

fn range(input: Input<'_>) -> Result<Option<XhttpWireValue>, ClientError> {
    let (min, max) = input.bounds()?;
    Ok(Some(Range(min, max)))
}

fn seconds(input: Input<'_>) -> Result<Option<XhttpWireValue>, ClientError> {
    let Input::Json(value) = input else {
        return Err(ClientError::InvalidSubscription);
    };
    let value = value.as_i64().ok_or(ClientError::InvalidSubscription)?;
    if !(-1..=i64::from(u32::MAX)).contains(&value) {
        return Err(ClientError::InvalidSubscription);
    }
    Ok(Some(Seconds(value)))
}

fn xmux(input: Input<'_>) -> Result<Option<XhttpWireValue>, ClientError> {
    let parse = |value: &Value| {
        let object = value.as_object().ok_or(ClientError::InvalidSubscription)?;
        parse_object(XMUX_FIELDS, object)
            .map(XhttpWireValue::Xmux)
            .map(Some)
    };
    match input {
        Input::Json(value) => parse(value),
        Input::Text(value) => parse(&crate::client_normalize::parse_json_value(value)?),
    }
}
