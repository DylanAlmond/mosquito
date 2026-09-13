use qrcode::{QrCode, render::svg};

use crate::QrError;

/// Render `text` as a QR code, as an SVG string the UI can drop inline.
/// SVG rather than PNG: crisp at any scale, styleable with CSS, and no
/// image-encoding dependency. Quiet zone is the standard 4-module
/// border — scanners expect it.
pub fn qr_svg(text: &str) -> Result<String, QrError> {
    let code = QrCode::new(text.as_bytes())?;
    
    Ok(code
        .render::<svg::Color>()
        .quiet_zone(true)
        .dark_color(svg::Color("#111111"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_svg_produces_svg_markup() {
        let svg = qr_svg("http://192.168.1.42:8472").unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
    }

    #[test]
    fn qr_svg_is_deterministic() {
        let a = qr_svg("http://192.168.1.42:8472").unwrap();
        let b = qr_svg("http://192.168.1.42:8472").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn qr_svg_differs_for_different_payloads() {
        let a = qr_svg("http://192.168.1.42:8472").unwrap();
        let b = qr_svg("http://192.168.1.42:9999").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn qr_svg_rejects_oversized_payload() {
        // A QR code tops out around 2953 bytes; far beyond any URL we make.
        let huge = "a".repeat(4000);
        assert!(qr_svg(&huge).is_err());
    }
}
