//! Server-side QR symbols (SVG) for pairing and club check-in screens.
use qrcode_generator::{
    qr::{Encoder, ErrorCorrection},
    Renderer,
};

pub fn svg_for_payload(payload: &str, size: u32) -> Result<String, String> {
    let symbol = Encoder::new(ErrorCorrection::Medium)
        .encode_text(payload)
        .map_err(|e| format!("encode: {e:?}"))?;
    Renderer::new(&symbol, size as usize)
        .to_svg_string(None::<&str>)
        .map_err(|e| format!("render: {e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_svg_markup() {
        let svg = svg_for_payload("hello", 128).expect("svg");
        assert!(svg.contains("<svg"));
        assert!(!svg.contains("qrserver"));
    }
}
