//! QR decoding shared by native input adapters.

use std::io::Cursor;

use image::{ImageReader, Limits};

pub const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_SIDE: u32 = 8192;
const MAX_SCAN_PIXELS: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Image,
    TooLarge,
    Missing,
    Multiple,
}

impl Error {
    pub fn key(self) -> vpn_presentation::localization::Key {
        use vpn_presentation::localization::Key;
        match self {
            Self::Image => Key::QrUnreadableImage,
            Self::TooLarge => Key::QrImageTooLarge,
            Self::Missing => Key::QrMissing,
            Self::Multiple => Key::QrMultiple,
        }
    }
}

pub fn image(bytes: &[u8]) -> Result<String, Error> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(Error::TooLarge);
    }
    let probe = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| Error::Image)?;
    let format = probe.format().ok_or(Error::Image)?;
    let (width, height) = probe.into_dimensions().map_err(|_| Error::Image)?;
    check_dimensions(width as usize, height as usize)?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|error| match error {
        image::ImageError::Limits(_) => Error::TooLarge,
        _ => Error::Image,
    })?;
    let luma = decoded.into_luma8();
    gray(luma.width() as usize, luma.height() as usize, luma.as_raw())
}

pub fn gray(width: usize, height: usize, pixels: &[u8]) -> Result<String, Error> {
    let count = check_dimensions(width, height)?;
    if pixels.len() != count {
        return Err(Error::Image);
    }
    decode(width, height, pixels)
}

pub fn check_dimensions(width: usize, height: usize) -> Result<usize, Error> {
    let count = width.checked_mul(height).ok_or(Error::TooLarge)?;
    if width > MAX_SIDE as usize || height > MAX_SIDE as usize || count > MAX_SCAN_PIXELS {
        return Err(Error::TooLarge);
    }
    if count == 0 {
        return Err(Error::Image);
    }
    Ok(count)
}

fn decode(width: usize, height: usize, pixels: &[u8]) -> Result<String, Error> {
    let mut decoder = quircs::Quirc::default();
    let mut texts = decoder
        .identify(width, height, pixels)
        .filter_map(Result::ok)
        .filter_map(|code| code.decode().ok())
        .filter_map(|decoded| String::from_utf8(decoded.payload).ok())
        .filter(|text| !vpn_presentation::import::payload(text).is_empty());
    let first = texts.next().ok_or(Error::Missing)?;
    if texts.any(|text| text != first) {
        return Err(Error::Multiple);
    }
    Ok(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_server_qr_in_a_file_and_a_raw_camera_frame() {
        let fixture = include_bytes!("tests/socks-qr.png");
        let expected = "socks5://example.test:1080#QR";
        assert_eq!(image(fixture).unwrap(), expected);
        let pixels = ::image::load_from_memory(fixture).unwrap().into_luma8();
        assert_eq!(
            gray(
                pixels.width() as usize,
                pixels.height() as usize,
                pixels.as_raw()
            )
            .unwrap(),
            expected
        );
        assert_eq!(gray(64, 64, &[255; 64 * 64]), Err(Error::Missing));
        assert_eq!(gray(8192, 8192, &[]), Err(Error::TooLarge));
    }
}
