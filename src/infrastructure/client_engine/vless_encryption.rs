//! Validate the pinned BlazeCore configuration format before a profile enters the catalog.

use std::iter::Peekable;
use std::str::Split;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use vpn_domain::client::ClientError;

pub(super) fn validate(setting: &str) -> Result<(), ClientError> {
    let mut blocks = setting.split('.').peekable();
    let [Some(method), Some(appearance), Some(rtt)] = [blocks.next(), blocks.next(), blocks.next()]
    else {
        return Err(ClientError::InvalidInput);
    };
    if method != "mlkem768x25519plus"
        || !matches!(appearance, "native" | "xorpub" | "random")
        || !matches!(rtt, "0rtt" | "1rtt")
    {
        return Err(ClientError::UnsupportedFeature);
    }
    validate_padding(&mut blocks)?;
    if blocks.peek().is_none() || !blocks.all(valid_key) {
        return Err(ClientError::InvalidInput);
    }
    Ok(())
}

fn validate_padding(blocks: &mut Peekable<Split<'_, char>>) -> Result<(), ClientError> {
    let padding = std::iter::from_fn(|| blocks.next_if(|block| block.len() < 20));
    let mut maximum = [0_u32; 2];
    for (index, block) in padding.enumerate() {
        let [probability, min, max] = padding_fields(block)?;
        if probability > 100 || min > max || (index == 0 && (probability != 100 || min < 35)) {
            return Err(ClientError::InvalidInput);
        }
        let total = &mut maximum[index % 2];
        *total = total.saturating_add(max);
    }
    let [bytes, milliseconds] = maximum;
    if bytes > 65_553 || milliseconds > 30_000 {
        return Err(ClientError::InvalidInput);
    }
    Ok(())
}

fn padding_fields(block: &str) -> Result<[u32; 3], ClientError> {
    let mut fields = block.split('-');
    let [Some(probability), Some(min), Some(max)] =
        std::array::from_fn(|_| fields.next().and_then(decimal))
    else {
        return Err(ClientError::InvalidInput);
    };
    if fields.next().is_some() {
        return Err(ClientError::InvalidInput);
    }
    Ok([probability, min, max])
}

fn decimal(value: &str) -> Option<u32> {
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.parse().ok();
    }
    None
}

fn valid_key(value: &str) -> bool {
    matches!(value.len(), 43 | 1579) && URL_SAFE_NO_PAD.decode(value).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_client_settings_before_use() {
        let key = URL_SAFE_NO_PAD.encode([42; 32]);
        for (value, expected) in [
            (
                "mlkem768x25519plus.native.0rtt".to_owned(),
                ClientError::InvalidInput,
            ),
            (
                format!("other.native.0rtt.{key}"),
                ClientError::UnsupportedFeature,
            ),
            (
                format!("mlkem768x25519plus.other.0rtt.{key}"),
                ClientError::UnsupportedFeature,
            ),
            (
                format!("mlkem768x25519plus.native.600s.{key}"),
                ClientError::UnsupportedFeature,
            ),
            (
                format!("mlkem768x25519plus.native.0rtt.100-34-50.{key}"),
                ClientError::InvalidInput,
            ),
            (
                format!("mlkem768x25519plus.native.0rtt.100-35-65554.{key}"),
                ClientError::InvalidInput,
            ),
            (
                format!("mlkem768x25519plus.native.0rtt.100-35-50.100-0-30001.{key}"),
                ClientError::InvalidInput,
            ),
            (
                format!("mlkem768x25519plus.native.0rtt.{}", "!".repeat(43)),
                ClientError::InvalidInput,
            ),
        ] {
            assert_eq!(validate(&value), Err(expected));
        }
    }
}
