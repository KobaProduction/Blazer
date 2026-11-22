//! Import entry points shared by desktop and mobile.

use std::borrow::Cow;

use crate::localization::{self, Key, Locale};
use crate::model::Intent;

pub use vpn_application::MAX_SUBSCRIPTION_BYTES as MAX_TEXT_BYTES;

pub const IMPORT_LINK_TEMPLATE: &str = "blazer://add/{url}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind {
    Subscription,
    Configuration,
}

pub fn detect(text: &str) -> ImportKind {
    let text = import_payload(text);
    if !text.chars().any(char::is_whitespace)
        && let Some((scheme, rest)) = text.split_once("://")
        && (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        && !rest
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("")
            .contains('@')
    {
        return ImportKind::Subscription;
    }
    ImportKind::Configuration
}

pub fn payload(text: &str) -> &str {
    text.trim().trim_start_matches('\u{feff}').trim_start()
}

pub fn import_link(text: &str) -> Option<Cow<'_, str>> {
    let text = payload(text);
    if text.len() > MAX_TEXT_BYTES {
        return None;
    }
    let (scheme, action) = text.split_once("://")?;
    let (action, value) = action.split_once('/')?;
    if !scheme.eq_ignore_ascii_case("blazer") || !action.eq_ignore_ascii_case("add") {
        return None;
    }
    let value = if value
        .split_once("://")
        .is_some_and(|(scheme, _)| valid_scheme(scheme))
    {
        Cow::Borrowed(value)
    } else {
        decode_component(value)?
    };
    let (scheme, target) = value.split_once("://")?;
    (valid_scheme(scheme)
        && !scheme.eq_ignore_ascii_case("blazer")
        && !target.is_empty()
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control()))
    .then_some(value)
}

fn decode_component(encoded: &str) -> Option<Cow<'_, str>> {
    if !encoded.contains('%') {
        return Some(Cow::Borrowed(encoded));
    }
    let mut bytes = encoded.bytes();
    let mut decoded = Vec::with_capacity(encoded.len());
    while let Some(byte) = bytes.next() {
        let decoded_byte = if byte == b'%' {
            let high = char::from(bytes.next()?).to_digit(16)?;
            let low = char::from(bytes.next()?).to_digit(16)?;
            u8::try_from(high * 16 + low).ok()?
        } else {
            byte
        };
        decoded.push(decoded_byte);
    }
    String::from_utf8(decoded).ok().map(Cow::Owned)
}

fn valid_scheme(scheme: &str) -> bool {
    scheme.starts_with(|character: char| character.is_ascii_alphabetic())
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+.-".contains(&byte))
}

fn import_payload(text: &str) -> Cow<'_, str> {
    import_link(text).unwrap_or_else(|| Cow::Borrowed(payload(text)))
}

impl ImportKind {
    pub fn default_title(self, locale: Locale) -> &'static str {
        let key = match self {
            Self::Subscription => Key::SubscriptionKind,
            Self::Configuration => Key::ImportedServers,
        };
        localization::text(locale, key)
    }

    pub fn intent(self, locale: Locale, title: &str, text: &str) -> Intent {
        let title = title.trim();
        let title = if title.is_empty() && self == Self::Configuration {
            self.default_title(locale)
        } else {
            title
        }
        .to_owned();
        let input = import_payload(text).into_owned();
        match self {
            Self::Subscription => Intent::SaveSubscription {
                subscription_uuid: None,
                title,
                url: input,
            },
            Self::Configuration => Intent::ImportProfiles { title, text: input },
        }
    }

    pub fn error_message(self, error: Key, input: &str) -> Key {
        match (self, error) {
            (Self::Subscription, Key::ParseFailed) => Key::ImportInvalidSubscription,
            (Self::Subscription, Key::InvalidInput) => Key::ImportInvalidUrl,
            (Self::Configuration, Key::ParseFailed) if starts_json(payload(input)) => {
                Key::ImportInvalidJson
            }
            (Self::Configuration, Key::ParseFailed) => Key::ImportInvalidConfiguration,
            (Self::Configuration, Key::Unsupported) => Key::ImportUnsupportedConfiguration,
            _ => error,
        }
    }
}

fn starts_json(input: &str) -> bool {
    input.starts_with('{')
        || input
            .strip_prefix('[')
            .is_some_and(|rest| rest.trim_start().starts_with(['{', '[', '"', ']']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_routes_single_web_urls_and_keeps_documents_local() {
        use ImportKind::{Configuration, Subscription};
        assert!(detect(" \u{feff}HTTPS://provider.example/link?key=demo ") == Subscription);
        assert!(detect("http://proxy:pass@example.test:8080") == Configuration);
        assert!(detect("vmess://YWJj") == Configuration);
        assert!(detect("c29ja3M1Oi8vZXhhbXBsZS50ZXN0OjEwODA=") == Configuration);
        assert!(detect(r#"{"outbounds":[{"host":"https://example.test"}]}"#) == Configuration);
        assert!(detect("https://example.test/sub\nsocks5://example.test:1080") == Configuration);
    }

    #[test]
    fn blazer_links_import_the_inner_url_without_rewriting_encoded_credentials() {
        let expected = "https://example.test/a%2Fb?token=x%2Fy&name=A+B";
        let raw = format!("blazer://add/{expected}");
        let encoded =
            "Blazer://ADD/https%3A%2F%2Fexample.test%2Fa%252Fb%3Ftoken%3Dx%252Fy%26name%3DA%2BB";
        for link in [&raw, encoded] {
            assert_eq!(detect(link), ImportKind::Subscription);
            assert!(matches!(
                ImportKind::Subscription.intent(Locale::English, "", link),
                Intent::SaveSubscription { url, title, .. } if url == expected && title.is_empty()
            ));
        }
        assert_eq!(
            detect("blazer://add/ss://sample"),
            ImportKind::Configuration
        );
        assert!(import_link("happ://add/https://example.test").is_none());
        assert!(import_link("blazer://connect").is_none());
        assert!(import_link("blazer://add/blazer://add/https://example.test").is_none());
        assert!(import_link("blazer://add/https%3A%2F%2Fexample.test%0A").is_none());
    }
}
