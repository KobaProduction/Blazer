//! Happ wire metadata and normalized HTTP endpoints.
use std::borrow::Cow;
use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use percent_encoding::percent_decode_str;
use reqwest::header::HeaderMap;
use url::{Host, Url};
use vpn_application::MAX_SUBSCRIPTION_BYTES;
use vpn_application::provider::FetchedSubscription;
use vpn_application::session::{
    ProviderDisplay, ProviderNotice, ProviderNoticeButton, ProviderNoticeColor, ProviderSort,
};
use vpn_domain::client::{ClientError, Secret};
use vpn_domain::provider::{
    MAX_METADATA_VALUE_BYTES, ProviderEndpoint, ProviderKey, ProviderMetadata, SubscriptionUsage,
};

const DISPLAY_TEXT_CHARS: usize = 200;
const BUTTON_LABEL_CHARS: usize = 25;
const PROFILE_TITLE_CHARS: usize = 25;
const EXPIRY_NOTICE_SECONDS: u64 = 3 * 24 * 60 * 60;

pub fn subscription_display(metadata: &ProviderMetadata, now_unix: u64) -> ProviderDisplay {
    use ProviderKey::*;

    let value = |key| display_value(metadata, key);
    ProviderDisplay {
        announcement: value(Announce).and_then(announcement),
        info: info_notice(metadata),
        expiry: expiry_notice(metadata, now_unix),
        support_url: value(SupportUrl).and_then(display_url),
        website_url: value(ProfileWebPageUrl).and_then(display_url),
        pin: value(SubscriptionPin).and_then(display_bool),
        keep_expanded: value(SubscriptionsCollapse).and_then(display_bool) == Some(false),
        expand_now: value(SubscriptionsExpandNow).and_then(display_bool) == Some(true),
        sort: match value(SubscriptionsSortType)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("without") => Some(ProviderSort::Original),
            Some("ping") => Some(ProviderSort::Latency),
            Some("alphabet") => Some(ProviderSort::Alphabetical),
            _ => None,
        },
        ping_icon: match value(PingResult).map(str::to_ascii_lowercase).as_deref() {
            Some("time") => Some(false),
            Some("icon") => Some(true),
            _ => None,
        },
    }
}

pub(crate) fn provider_title(url: &str, metadata: &ProviderMetadata) -> Option<String> {
    [ProviderKey::ProfileTitle, ProviderKey::Name]
        .into_iter()
        .find_map(|key| display_value(metadata, key).and_then(parse_title))
        .or_else(|| url_title(url))
}

fn url_title(url: &str) -> Option<String> {
    let url = Url::parse(url).ok()?;
    let mut names = url.query()?.split('&').filter_map(|parameter| {
        let (key, value) = parameter.split_once('=').unwrap_or((parameter, ""));
        let key = percent_decode_str(key).decode_utf8().ok()?;
        key.eq_ignore_ascii_case("name").then_some(value)
    });
    let name = names.next()?;
    if names.next().is_some() {
        return None;
    }
    let decoded = name.replace('+', " ");
    let decoded = percent_decode_str(&decoded).decode_utf8().ok()?;
    parse_title(decoded.trim())
}

fn parse_title(text: &str) -> Option<String> {
    let decoded;
    let text = if let Some(encoded) = text.strip_prefix("base64:") {
        decoded = decode_base64(encoded).ok()?;
        decoded.trim()
    } else {
        text
    };
    bounded_text(text, PROFILE_TITLE_CHARS)
}

fn display_value(metadata: &ProviderMetadata, key: ProviderKey) -> Option<&str> {
    metadata.value(key).map(|value| value.expose().trim())
}

fn display_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

fn announcement(text: &str) -> Option<String> {
    let text = display_message(text)?;
    Some(text.trim().chars().take(DISPLAY_TEXT_CHARS).collect())
}

fn display_message(text: &str) -> Option<Cow<'_, str>> {
    let text = decode_display_text(text)?;
    let text = if text.contains('\r') {
        Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        text
    };
    (!text.trim().is_empty()
        && !text
            .chars()
            .any(|ch| !matches!(ch, '\n' | '\t') && invalid_display_char(ch)))
    .then_some(text)
}

fn decode_display_text(text: &str) -> Option<Cow<'_, str>> {
    if let Some((kind, encoded)) = text.split_once(':')
        && kind.eq_ignore_ascii_case("base64")
    {
        Some(Cow::Owned(decode_base64(encoded).ok()?))
    } else {
        Some(Cow::Borrowed(text))
    }
}

fn bounded_text(text: &str, max_chars: usize) -> Option<String> {
    (!text.is_empty()
        && text.chars().count() <= max_chars
        && !text.chars().any(invalid_display_char))
    .then(|| text.to_owned())
}

fn invalid_display_char(ch: char) -> bool {
    ch.is_control()
        || matches!(ch, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

fn display_url(text: &str) -> Option<String> {
    let (scheme, authority) = text.split_once("://")?;
    if authority.starts_with('/')
        || !["https", "http"]
            .iter()
            .any(|valid| scheme.eq_ignore_ascii_case(valid))
        || text.contains('\\')
        || text
            .chars()
            .any(|ch| ch.is_whitespace() || invalid_display_char(ch))
    {
        return None;
    }
    let endpoint = endpoint(text).ok()?;
    Some(endpoint.url.expose().to_owned())
}

fn info_notice(metadata: &ProviderMetadata) -> Option<ProviderNotice> {
    use ProviderKey::*;

    let value = |key| display_value(metadata, key);
    let message = value(SubInfoText).and_then(display_message)?;
    let text = message.trim();
    if text == "0" || text.chars().count() > DISPLAY_TEXT_CHARS {
        return None;
    }
    let color = match value(SubInfoColor).map(str::to_ascii_lowercase).as_deref() {
        Some("red") => ProviderNoticeColor::Red,
        Some("green") => ProviderNoticeColor::Green,
        _ => ProviderNoticeColor::Blue,
    };
    let button = value(SubInfoButtonText)
        .and_then(decode_display_text)
        .and_then(|label| bounded_text(label.trim_matches(' '), BUTTON_LABEL_CHARS))
        .zip(value(SubInfoButtonLink).and_then(display_url))
        .map(|(label, url)| ProviderNoticeButton { label, url });
    Some(ProviderNotice {
        text: text.to_owned(),
        color,
        button,
    })
}

fn expiry_notice(metadata: &ProviderMetadata, now_unix: u64) -> Option<ProviderNotice> {
    if display_value(metadata, ProviderKey::SubExpire).and_then(display_bool) != Some(true)
        || subscription_usage(metadata)?.expires_at_unix?
            > now_unix.saturating_add(EXPIRY_NOTICE_SECONDS)
    {
        return None;
    }
    let button = display_value(metadata, ProviderKey::SubExpireButtonLink)
        .and_then(display_url)
        .map(|url| ProviderNoticeButton {
            label: String::new(),
            url,
        });
    Some(ProviderNotice {
        text: String::new(),
        color: ProviderNoticeColor::Red,
        button,
    })
}

pub fn subscription_usage(metadata: &ProviderMetadata) -> Option<SubscriptionUsage> {
    let text = metadata.value(ProviderKey::SubscriptionUserinfo)?.expose();
    let field = |name| usage_field(text, name);
    let used_bytes = field("upload")
        .zip(field("download"))
        .and_then(|(upload, download)| upload.checked_add(download));
    let total_bytes = field("total");
    let expires_at_unix = field("expire").filter(|&unix| unix != 0);
    let usage = SubscriptionUsage {
        used_bytes,
        total_bytes,
        expires_at_unix,
    };
    (usage != SubscriptionUsage::default()).then_some(usage)
}

fn usage_field(text: &str, key: &str) -> Option<u64> {
    let mut values = text
        .split(';')
        .filter_map(|entry| entry.split_once('='))
        .filter_map(|(name, value)| {
            name.trim()
                .eq_ignore_ascii_case(key)
                .then_some(value.trim())
        });
    let value = values.next()?;
    if values.next().is_some() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

pub fn endpoint(text: &str) -> Result<ProviderEndpoint, ClientError> {
    if text.len() > MAX_METADATA_VALUE_BYTES || text.chars().any(char::is_control) {
        return Err(ClientError::InvalidUrl);
    }
    let url = Url::parse(text).map_err(|_| ClientError::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.as_str().len() > MAX_METADATA_VALUE_BYTES
    {
        return Err(ClientError::InvalidUrl);
    }
    Ok(ProviderEndpoint {
        origin: url.origin().ascii_serialization(),
        url: Secret::new(String::from(url)),
    })
}

pub(crate) fn valid_etag(value: &str) -> bool {
    let opaque = value.strip_prefix("W/").unwrap_or(value);
    value.len() <= MAX_METADATA_VALUE_BYTES
        && opaque
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .is_some_and(|tag| tag.bytes().all(|byte| matches!(byte, 0x21 | 0x23..=0x7e)))
}

pub fn replace_domain(
    primary: &ProviderEndpoint,
    domain: &str,
) -> Result<ProviderEndpoint, ClientError> {
    if domain.is_empty()
        || domain
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control() || "/:@?#\\%".contains(ch))
    {
        return Err(ClientError::InvalidUrl);
    }
    let Host::Domain(host) = Host::parse(domain).map_err(|_| ClientError::InvalidUrl)? else {
        return Err(ClientError::InvalidUrl);
    };
    let mut url = Url::parse(primary.url.expose()).map_err(|_| ClientError::InvalidUrl)?;
    url.set_host(Some(&host))
        .map_err(|_| ClientError::InvalidUrl)?;
    endpoint(url.as_str())
}

pub fn parse_document(
    url: &Url,
    headers: &HeaderMap,
    body: Vec<u8>,
) -> Result<FetchedSubscription, ClientError> {
    let mut metadata = ProviderMetadata::default();
    if let Some(fragment) = url
        .fragment()
        .and_then(|fragment| fragment.strip_prefix('?'))
    {
        for (key, value) in url::form_urlencoded::parse(fragment.as_bytes()) {
            if key.eq_ignore_ascii_case("providerid") {
                insert(&mut metadata, ProviderKey::ProviderId, value.as_ref())?;
            }
        }
    }
    let mut body_metadata = ProviderMetadata::default();
    let body = strip_metadata(body, &mut body_metadata)?;
    let mut header_metadata = ProviderMetadata::default();
    for (name, header) in headers {
        let Some(key) = ProviderKey::from_wire(name.as_str()) else {
            continue;
        };
        let value = match std::str::from_utf8(header.as_bytes()) {
            Ok(value) => value,
            Err(_) if is_projected_display_key(key) => continue,
            Err(_) => return Err(ClientError::InvalidSubscription),
        };
        insert(&mut header_metadata, key, value.trim())?;
    }
    // Prefer the transport header over the body comment, then the URL fragment.
    metadata.values.extend(body_metadata.values);
    metadata.values.extend(header_metadata.values);
    validate_with_optional_display(&mut metadata)?;
    validate_action_values(&metadata)?;
    Ok(FetchedSubscription { body, metadata })
}

fn insert(
    metadata: &mut ProviderMetadata,
    key: ProviderKey,
    value: &str,
) -> Result<(), ClientError> {
    if value.is_empty() && key != ProviderKey::SubInfoText {
        return Ok(());
    }
    if is_projected_display_key(key) {
        insert_display_metadata(metadata, key, value);
        return Ok(());
    }
    if value.len() > MAX_METADATA_VALUE_BYTES {
        return Err(ClientError::PayloadTooLarge);
    }
    if let Some(previous) = metadata.value(key) {
        if previous.expose() != value {
            return Err(ClientError::InvalidSubscription);
        }
        return Ok(());
    }
    metadata.values.insert(key, Secret::new(value));
    validate_with_optional_display(metadata)
}

fn validate_with_optional_display(metadata: &mut ProviderMetadata) -> Result<(), ClientError> {
    match metadata.validate() {
        Err(ClientError::PayloadTooLarge) => {
            metadata
                .values
                .retain(|key, _| !is_projected_display_key(*key));
            metadata.validate()
        }
        result => result,
    }
}

fn is_projected_display_key(key: ProviderKey) -> bool {
    use ProviderKey::*;

    matches!(
        key,
        Announce
            | ProfileTitle
            | Name
            | SupportUrl
            | ProfileWebPageUrl
            | SubInfoColor
            | SubInfoText
            | SubInfoButtonText
            | SubInfoButtonLink
            | SubExpire
            | SubExpireButtonLink
            | SubscriptionPin
            | SubscriptionsCollapse
            | SubscriptionsExpandNow
            | SubscriptionsSortType
            | PingResult
    )
}

fn insert_display_metadata(metadata: &mut ProviderMetadata, key: ProviderKey, value: &str) {
    if value.len() > MAX_METADATA_VALUE_BYTES
        || value.chars().any(invalid_display_char)
        || matches!(key, ProviderKey::ProfileTitle | ProviderKey::Name)
            && parse_title(value).is_none()
        || metadata.values.contains_key(&key)
    {
        return;
    }
    metadata.values.insert(key, Secret::new(value));
    if metadata.validate().is_err() {
        metadata.values.remove(&key);
    }
}

fn strip_metadata(body: Vec<u8>, metadata: &mut ProviderMetadata) -> Result<Vec<u8>, ClientError> {
    let text = String::from_utf8(body).map_err(|_| ClientError::InvalidSubscription)?;
    let text = text.trim().trim_start_matches('\u{feff}').trim_start();
    if text.starts_with(['{', '[']) {
        return Ok(text.as_bytes().to_vec());
    }
    let decoded;
    let text = if text.starts_with('#') || text.contains("://") || text.is_empty() {
        text
    } else {
        decoded = decode_base64(text)?;
        let text = decoded.trim().trim_start_matches('\u{feff}').trim_start();
        if text.starts_with(['{', '[']) {
            return Ok(text.as_bytes().to_vec());
        }
        text
    };
    let mut configuration = Vec::new();
    let mut configuration_started = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(comment) = trimmed.strip_prefix('#')
            && !configuration_started
        {
            parse_comment(comment.trim(), metadata)?;
        } else {
            configuration_started |= !trimmed.is_empty();
            if !configuration.is_empty() {
                configuration.push(b'\n');
            }
            configuration.extend_from_slice(line.as_bytes());
        }
    }
    Ok(configuration)
}

fn parse_comment(comment: &str, metadata: &mut ProviderMetadata) -> Result<(), ClientError> {
    if let Some(separator) = comment.find(|ch: char| ch == ':' || ch.is_whitespace())
        && let Some(key) = ProviderKey::from_wire(&comment[..separator])
    {
        let value = comment[separator..].trim_start();
        let value = value.strip_prefix(':').unwrap_or(value);
        insert(metadata, key, value.trim())?;
    }
    Ok(())
}

fn decode_base64(text: &str) -> Result<String, ClientError> {
    let compact: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    for codec in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD] {
        if let Ok(decoded) = codec.decode(&compact) {
            if decoded.len() > MAX_SUBSCRIPTION_BYTES {
                return Err(ClientError::PayloadTooLarge);
            }
            return String::from_utf8(decoded).map_err(|_| ClientError::InvalidSubscription);
        }
    }
    Err(ClientError::InvalidSubscription)
}

fn validate_action_values(metadata: &ProviderMetadata) -> Result<(), ClientError> {
    if let Some(value) = metadata.value(ProviderKey::SubscriptionRequestTimeout)
        && !value
            .expose()
            .parse::<u64>()
            .is_ok_and(|seconds| (5..=15).contains(&seconds))
    {
        return Err(ClientError::InvalidSubscription);
    }
    Ok(())
}

pub fn stored_metadata(raw: BTreeMap<String, String>) -> Result<ProviderMetadata, ClientError> {
    let mut metadata = ProviderMetadata::default();
    for (key, value) in raw {
        if let Some(key) = ProviderKey::from_wire(&key) {
            insert(&mut metadata, key, &value)?;
        }
    }
    validate_action_values(&metadata)?;
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vpn_application::provider::check_security;

    #[test]
    fn subscription_display_projects_known_values_and_hides_link_values_from_debug() {
        let raw = [
            ("announce", "base64:0J/RgNC40LLQtdGC"),
            (
                "support-url",
                "https://provider.invalid/support?ticket=private-value",
            ),
            ("profile-web-page-url", "http://provider.invalid/info"),
            ("sub-info-text", "Planned maintenance"),
            ("sub-info-color", "green"),
            ("sub-info-button-text", "Details"),
            ("sub-info-button-link", "https://provider.invalid/details"),
            ("subscription-pin", "0"),
            ("subscriptions-collapse", "false"),
            ("subscriptions-expand-now", "1"),
            ("subscriptions-sort-type", "ping"),
            ("ping-result", "icon"),
        ];
        let metadata =
            stored_metadata(raw.into_iter().map(|(k, v)| (k.into(), v.into())).collect()).unwrap();
        let display = subscription_display(&metadata, 10);
        assert!(!format!("{display:?}").contains("private-value"));
        assert_eq!(display.announcement.as_deref(), Some("Привет"));
        assert_eq!(display.support_url.as_deref(), Some(raw[1].1));
        assert_eq!(display.website_url.as_deref(), Some(raw[2].1));
        assert_eq!(display.pin, Some(false));
        assert!(display.keep_expanded);
        assert!(display.expand_now);
        assert_eq!(display.sort, Some(ProviderSort::Latency));
        assert_eq!(display.ping_icon, Some(true));
        let info = display.info.unwrap();
        assert_eq!(info.text, "Planned maintenance");
        assert_eq!(info.color, ProviderNoticeColor::Green);
        let button = info.button.unwrap();
        assert_eq!(button.label, "Details");
        assert_eq!(button.url, raw[6].1);
    }

    #[test]
    fn provider_notice_text_decodes_explicit_base64_from_headers_and_body() {
        use reqwest::header::HeaderValue;

        let mut headers = HeaderMap::new();
        for (name, text) in [
            ("announce", "Новости 🧪\r\n消息\nNews\rMore\tinfo"),
            ("sub-info-text", "Плановые работы\r\nStatus\tupdate"),
        ] {
            let encoded = format!("BASE64:{}", STANDARD.encode(text));
            headers.insert(name, HeaderValue::from_str(&encoded).unwrap());
        }
        let button = URL_SAFE_NO_PAD.encode("Подробнее");
        let body = format!(
            "#sub-info-button-text: base64:{button}\n#sub-info-button-link: https://provider.invalid/status\nvless://sample"
        );
        let source = Url::parse("https://provider.invalid/subscription").unwrap();
        let fetched = parse_document(&source, &headers, body.into_bytes()).unwrap();
        let display = subscription_display(&fetched.metadata, 0);
        assert_eq!(fetched.body, b"vless://sample");
        assert_eq!(
            display.announcement.as_deref(),
            Some("Новости 🧪\n消息\nNews\nMore\tinfo")
        );
        let info = display.info.unwrap();
        assert_eq!(info.text, "Плановые работы\nStatus\tupdate");
        let button = info.button.unwrap();
        assert_eq!(button.label, "Подробнее");
        assert_eq!(button.url, "https://provider.invalid/status");
    }

    #[test]
    fn decoded_notice_text_obeys_display_validation() {
        for label in ["я".repeat(BUTTON_LABEL_CHARS + 1), "\tПодробнее".to_owned()] {
            let metadata = stored_metadata(
                [
                    (
                        "announce",
                        format!("base64:{}", STANDARD.encode("unsafe\u{202e}text")),
                    ),
                    ("sub-info-text", "QUJD".to_owned()),
                    (
                        "sub-info-button-text",
                        format!("base64:{}", STANDARD.encode(label)),
                    ),
                    (
                        "sub-info-button-link",
                        "https://provider.invalid/status".to_owned(),
                    ),
                ]
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
            )
            .unwrap();
            let display = subscription_display(&metadata, 0);
            assert!(display.announcement.is_none());
            let info = display.info.unwrap();
            assert_eq!(info.text, "QUJD");
            assert!(info.button.is_none());
        }
    }

    #[test]
    fn subscription_expiry_notice_starts_exactly_three_days_before_expiry() {
        let expires = 400_000_u64;
        let metadata = stored_metadata(BTreeMap::from([
            (
                "subscription-userinfo".to_owned(),
                format!("expire={expires}; total=0"),
            ),
            ("sub-expire".to_owned(), "true".to_owned()),
            (
                "sub-expire-button-link".to_owned(),
                "https://provider.invalid/renew".to_owned(),
            ),
            ("sub-info-text".to_owned(), "0".to_owned()),
        ]))
        .unwrap();
        assert!(
            subscription_display(&metadata, expires - EXPIRY_NOTICE_SECONDS - 1)
                .expiry
                .is_none()
        );
        for now in [expires - EXPIRY_NOTICE_SECONDS, expires, expires + 1] {
            let display = subscription_display(&metadata, now);
            assert!(display.info.is_none());
            let notice = display.expiry.unwrap();
            assert_eq!(notice.color, ProviderNoticeColor::Red);
            assert!(notice.text.is_empty());
            let button = notice.button.unwrap();
            assert!(button.label.is_empty());
            assert_eq!(button.url, "https://provider.invalid/renew");
        }
    }

    #[test]
    fn invalid_decoration_does_not_reject_subscription_profiles() {
        use reqwest::header::HeaderValue;

        let mut headers = HeaderMap::new();
        headers.insert("sub-info-text", HeaderValue::from_bytes(b"\xff").unwrap());
        headers.insert(
            "support-url",
            HeaderValue::from_static("https://user:pass@provider.invalid/"),
        );
        headers.insert(
            "profile-web-page-url",
            HeaderValue::from_static("javascript:noop()"),
        );
        headers.insert("announce", HeaderValue::from_static("base64:%%%"));
        let body = format!(
            "#sub-info-button-text: {}\nvless://example",
            "x".repeat(MAX_METADATA_VALUE_BYTES + 1)
        );
        let url = Url::parse("https://provider.invalid/subscription").unwrap();
        let fetched = parse_document(&url, &headers, body.into_bytes()).unwrap();
        assert_eq!(fetched.body, b"vless://example");
        assert_eq!(
            subscription_display(&fetched.metadata, 10),
            ProviderDisplay::default()
        );
    }

    #[test]
    fn domain_migration_validates_hostname_and_preserves_other_url_components() {
        let primary =
            endpoint("https://example.invalid:8443/sub/%2F?k=opaque#?providerid=one").unwrap();
        let target = replace_domain(&primary, "replacement.invalid").unwrap();
        assert_eq!(target.origin, "https://replacement.invalid:8443");
        assert_eq!(
            target.url.expose(),
            "https://replacement.invalid:8443/sub/%2F?k=opaque#?providerid=one"
        );
        assert_eq!(
            replace_domain(&primary, "replacement.invalid@third.invalid"),
            Err(ClientError::InvalidUrl)
        );
        let insecure = endpoint("http://replacement.invalid/sub").unwrap();
        assert_eq!(
            check_security(&primary, &insecure),
            Err(ClientError::InvalidUrl)
        );
    }
}
