use std::collections::BTreeMap;
use std::num::NonZeroU32;

use super::{ClientError, XhttpMode};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct XhttpPacketOptions {
    pub max_post_bytes: Option<XhttpPacketRange>,
    pub min_post_interval_ms: Option<XhttpPacketRange>,
    pub wire: XhttpWireOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum XhttpWireField {
    UplinkMethod,
    UplinkPlacement,
    UplinkKey,
    UplinkChunkSize,
    SessionTable,
    SessionLength,
    PaddingBytes,
    PaddingObfuscation,
    PaddingKey,
    PaddingHeader,
    PaddingPlacement,
    PaddingMethod,
    Xmux,
    MaxConnections,
    MaxConcurrency,
    MaxConnectionReuses,
    MaxHttpRequests,
    MaxHttpReuseSeconds,
    HttpKeepAliveSeconds,
}

pub type XhttpWireOptions = BTreeMap<XhttpWireField, XhttpWireValue>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XhttpWireValue {
    Text(String),
    Range(u32, u32),
    Flag(bool),
    Seconds(i64),
    Xmux(XhttpWireOptions),
}

impl XhttpPacketOptions {
    pub fn validate_mode(&self, mode: XhttpMode) -> Result<(), ClientError> {
        if mode == XhttpMode::PacketUp {
            return Ok(());
        }
        if matches!(self.wire.get(&XhttpWireField::UplinkMethod), Some(XhttpWireValue::Text(method)) if method == "GET")
        {
            return Err(ClientError::UnsupportedXhttpUploadMethod);
        }
        if matches!(self.wire.get(&XhttpWireField::UplinkPlacement), Some(XhttpWireValue::Text(placement)) if placement == "header" || placement == "cookie")
        {
            return Err(ClientError::UnsupportedFeature);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XhttpPacketRange {
    min: NonZeroU32,
    max: NonZeroU32,
}

impl XhttpPacketRange {
    pub fn new(min: u32, max: u32) -> Result<Self, ClientError> {
        if min > max {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self {
            min: NonZeroU32::new(min).ok_or(ClientError::InvalidInput)?,
            max: NonZeroU32::new(max).ok_or(ClientError::InvalidInput)?,
        })
    }

    pub fn bounds(self) -> (u32, u32) {
        (self.min.get(), self.max.get())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum XhttpPlacement {
    #[default]
    Path,
    Query,
    Header,
    Cookie,
}

impl XhttpPlacement {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Query => "query",
            Self::Header => "header",
            Self::Cookie => "cookie",
        }
    }
}

impl std::str::FromStr for XhttpPlacement {
    type Err = ClientError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "" | "path" => Ok(Self::Path),
            "query" => Ok(Self::Query),
            "header" => Ok(Self::Header),
            "cookie" => Ok(Self::Cookie),
            _ => Err(ClientError::UnsupportedFeature),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct XhttpMetadata {
    pub session_id_placement: XhttpPlacement,
    pub session_id_key: Option<String>,
    pub seq_placement: XhttpPlacement,
    pub seq_key: Option<String>,
}

impl XhttpMetadata {
    pub fn is_default(&self) -> bool {
        self.session_id_placement == XhttpPlacement::Path
            && self.seq_placement == XhttpPlacement::Path
    }

    pub fn validate(&self) -> Result<(), ClientError> {
        let session = self
            .session_id_key
            .as_deref()
            .unwrap_or(match self.session_id_placement {
                XhttpPlacement::Header => "X-Session",
                _ => "x_session",
            });
        let sequence = self.seq_key.as_deref().unwrap_or(match self.seq_placement {
            XhttpPlacement::Header => "X-Seq",
            _ => "x_seq",
        });
        validate_key(self.session_id_placement, session)?;
        validate_key(self.seq_placement, sequence)?;
        if self.session_id_placement == self.seq_placement {
            let same_key = match self.seq_placement {
                XhttpPlacement::Path => false,
                XhttpPlacement::Header => session.eq_ignore_ascii_case(sequence),
                _ => session == sequence,
            };
            if same_key {
                return Err(ClientError::InvalidInput);
            }
        }
        Ok(())
    }
}

fn validate_key(placement: XhttpPlacement, key: &str) -> Result<(), ClientError> {
    if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
        return Err(ClientError::InvalidInput);
    }
    if matches!(placement, XhttpPlacement::Header | XhttpPlacement::Cookie)
        && !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
    {
        return Err(ClientError::InvalidInput);
    }
    if placement == XhttpPlacement::Header
        && matches!(
            key.to_ascii_lowercase().as_str(),
            "host"
                | "keep-alive"
                | "proxy-connection"
                | "content-length"
                | "transfer-encoding"
                | "connection"
                | "upgrade"
                | "te"
                | "trailer"
                | "referer"
                | "cookie"
                | "content-type"
        )
    {
        return Err(ClientError::UnsupportedFeature);
    }
    Ok(())
}
