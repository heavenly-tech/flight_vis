//! Viewport-relative check-in QR placement for club kiosk screens.

pub const KIOSK_QR_MIN: f64 = 0.18;
pub const KIOSK_QR_MAX: f64 = 0.50;
pub const KIOSK_QR_MARGIN: f64 = 0.02;
pub const DEFAULT_PREVIEW_ASPECT: f64 = 16.0 / 9.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KioskQrLayout {
    pub x: f64,
    pub y: f64,
    pub size: f64,
}

/// Width/height of the square QR in normalized viewport fractions.
pub fn qr_footprint_fractions(aspect: f64, size: f64) -> (f64, f64) {
    let aspect = aspect.max(0.05);
    let (w, h) = if aspect >= 1.0 {
        (aspect, 1.0)
    } else {
        (1.0, 1.0 / aspect)
    };
    let short = w.min(h);
    (size * short / w, size * short / h)
}

/// Clamp x/y/size so the square stays inside the viewport (PATCH policy: clamp, never 400).
pub fn clamp_kiosk_qr_layout(x: f64, y: f64, size: f64, aspect: f64) -> KioskQrLayout {
    let size = size.clamp(KIOSK_QR_MIN, KIOSK_QR_MAX);
    let (sw, sh) = qr_footprint_fractions(aspect, size);
    let max_x = (1.0 - sw).max(0.0);
    let max_y = (1.0 - sh).max(0.0);
    KioskQrLayout {
        x: x.clamp(0.0, max_x),
        y: y.clamp(0.0, max_y),
        size,
    }
}

pub fn default_kiosk_qr_layout(aspect: f64) -> KioskQrLayout {
    let size = 0.22;
    let (sw, sh) = qr_footprint_fractions(aspect, size);
    KioskQrLayout {
        x: (1.0 - sw - KIOSK_QR_MARGIN).max(0.0),
        y: (1.0 - sh - KIOSK_QR_MARGIN).max(0.0),
        size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforces_min_max_size() {
        let a = clamp_kiosk_qr_layout(0.0, 0.0, 0.05, DEFAULT_PREVIEW_ASPECT);
        assert_eq!(a.size, KIOSK_QR_MIN);
        let b = clamp_kiosk_qr_layout(0.0, 0.0, 0.99, DEFAULT_PREVIEW_ASPECT);
        assert_eq!(b.size, KIOSK_QR_MAX);
    }

    #[test]
    fn stays_inside_viewport() {
        let l = clamp_kiosk_qr_layout(0.95, 0.95, 0.4, DEFAULT_PREVIEW_ASPECT);
        let (sw, sh) = qr_footprint_fractions(DEFAULT_PREVIEW_ASPECT, l.size);
        assert!(l.x + sw <= 1.0 + 1e-9);
        assert!(l.y + sh <= 1.0 + 1e-9);
    }

    #[test]
    fn default_is_bottom_right() {
        let d = default_kiosk_qr_layout(DEFAULT_PREVIEW_ASPECT);
        let (sw, sh) = qr_footprint_fractions(DEFAULT_PREVIEW_ASPECT, d.size);
        assert!(d.x + sw >= 0.95);
        assert!(d.y + sh >= 0.95);
    }
}
