//! Turn a captured frame into an image the MCP client can show.
//!
//! The plugin captures the Studio viewport and sends back the raw RGBA pixels (base64),
//! the width, and the height. The bytes-to-image step is here, where it is unit-tested:
//! decode the pixels, drop the alpha (JPEG has none), encode JPEG, and base64 that for an
//! MCP image content block. The capture itself needs a real Studio renderer; this part
//! does not, so it is verified without one.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use jpeg_encoder::{ColorType, Encoder};
use serde_json::Value;

const JPEG_QUALITY: u8 = 85;

/// Encode a capture payload (`{ width, height, data }`, data = base64 RGBA) into a
/// base64 JPEG. Returns a human-readable error if the payload is malformed.
pub fn encode_capture(data: &Value) -> Result<String, String> {
    let width = dimension(data, "width")?;
    let height = dimension(data, "height")?;
    let encoded = data
        .get("data")
        .and_then(Value::as_str)
        .ok_or("capture is missing pixel data")?;
    let rgba = STANDARD
        .decode(encoded)
        .map_err(|e| format!("capture pixels were not valid base64: {e}"))?;

    let expected = width as usize * height as usize * 4;
    if rgba.len() != expected {
        return Err(format!(
            "capture is {} pixel bytes, expected {expected} for {width}x{height}",
            rgba.len()
        ));
    }

    // JPEG has no alpha channel, so drop it.
    let rgb: Vec<u8> = rgba
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();

    let mut jpeg = Vec::new();
    Encoder::new(&mut jpeg, JPEG_QUALITY)
        .encode(&rgb, width, height, ColorType::Rgb)
        .map_err(|e| format!("jpeg encode failed: {e}"))?;

    Ok(STANDARD.encode(&jpeg))
}

fn dimension(data: &Value, key: &str) -> Result<u16, String> {
    let value = data
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("capture is missing {key}"))?;
    u16::try_from(value).map_err(|_| format!("capture {key} {value} is out of range"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_rgba(width: usize, height: usize, pixel: [u8; 4]) -> String {
        let bytes: Vec<u8> = std::iter::repeat_n(pixel, width * height)
            .flatten()
            .collect();
        STANDARD.encode(bytes)
    }

    #[test]
    fn encodes_rgba_to_a_real_jpeg() {
        let data = serde_json::json!({
            "width": 4, "height": 4, "data": solid_rgba(4, 4, [200, 30, 30, 255]),
        });
        let out = encode_capture(&data).expect("encodes");
        let jpeg = STANDARD.decode(out).expect("output is base64");
        // JPEG starts with SOI (FF D8) and ends with EOI (FF D9).
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        assert_eq!(&jpeg[jpeg.len() - 2..], &[0xFF, 0xD9]);
    }

    #[test]
    fn rejects_pixel_count_mismatch() {
        let data = serde_json::json!({
            "width": 4, "height": 4, "data": solid_rgba(2, 2, [0, 0, 0, 255]),
        });
        assert!(encode_capture(&data).is_err());
    }

    #[test]
    fn rejects_bad_base64() {
        let data = serde_json::json!({ "width": 1, "height": 1, "data": "not base64!!" });
        assert!(encode_capture(&data).is_err());
    }
}
