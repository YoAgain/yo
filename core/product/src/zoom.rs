//! The held viewer's zoom: a finger holding a bar's media sign and drawn down out of the bar magnifies the photo or
//! video around itself, the further below the bar the more (up to `MOST`, approached softly); moving it then pans,
//! as a magnifying glass would; back at the bar it is the picture as it was. Points (or dp) in screen coordinates.

/// The scale approached, never passed.
pub const MOST: f64 = 3.0;
/// Below the bar by this much before it starts: a finger trembling at the bar's edge does not zoom.
pub const SLACK: f64 = 6.0;

/// The picture scaled by `scale` around its center, then moved by `x`, `y`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zoom {
    pub scale: f64,
    pub x: f64,
    pub y: f64,
}

impl Zoom {
    pub const NONE: Zoom = Zoom { scale: 1.0, x: 0.0, y: 0.0 };
}

/// The zoom for a finger at `finger_x`, `finger_y`, the bar's lower edge at `bar_bottom`, the picture filling a
/// `width` × `height` screen. The point under the finger stays under it, so the picture never shows its edges.
pub fn hold_zoom(finger_x: f64, finger_y: f64, bar_bottom: f64, width: f64, height: f64) -> Zoom {
    let values = [finger_x, finger_y, bar_bottom, width, height];
    if values.iter().any(|v| !v.is_finite()) || width <= 0.0 || height <= 0.0 {
        return Zoom::NONE;
    }
    let below = finger_y - bar_bottom - SLACK;
    if below <= 0.0 {
        return Zoom::NONE;
    }
    // The room below the bar sets the pace: a bar low on the screen zooms over a shorter way.
    let reach = ((height - bar_bottom) * 0.45).clamp(80.0, 220.0);
    let scale = 1.0 + (MOST - 1.0) * (1.0 - (-below / reach).exp());
    let x = finger_x.clamp(0.0, width);
    let y = finger_y.clamp(0.0, height);
    Zoom { scale, x: (x - width / 2.0) * (1.0 - scale), y: (y - height / 2.0) * (1.0 - scale) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f64 = 400.0;
    const H: f64 = 800.0;

    #[test]
    fn at_or_above_the_bar_nothing() {
        assert_eq!(hold_zoom(200.0, 300.0, 300.0, W, H), Zoom::NONE);
        assert_eq!(hold_zoom(200.0, 300.0 + SLACK, 300.0, W, H), Zoom::NONE);
        assert_eq!(hold_zoom(50.0, 100.0, 300.0, W, H), Zoom::NONE);
    }

    #[test]
    fn grows_with_the_distance_and_stays_under_the_cap() {
        let mut last = 1.0;
        for step in 1..200 {
            let zoom = hold_zoom(200.0, 300.0 + SLACK + step as f64 * 5.0, 300.0, W, H);
            assert!(zoom.scale > last && zoom.scale < MOST, "{step}: {zoom:?}");
            last = zoom.scale;
        }
        assert!(last > 2.9);
        // Smooth from the start: a little below is a little zoom.
        assert!(hold_zoom(200.0, 300.0 + SLACK + 1.0, 300.0, W, H).scale < 1.05);
    }

    #[test]
    fn the_point_under_the_finger_stays() {
        for (fx, fy) in [(10.0, 400.0), (200.0, 500.0), (390.0, 790.0), (0.0, 800.0)] {
            let zoom = hold_zoom(fx, fy, 300.0, W, H);
            // The picture's point under the finger, before: (fx, fy); after the zoom it lands here.
            let lands_x = W / 2.0 + zoom.scale * (fx - W / 2.0) + zoom.x;
            let lands_y = H / 2.0 + zoom.scale * (fy - H / 2.0) + zoom.y;
            assert!((lands_x - fx).abs() < 1e-9 && (lands_y - fy).abs() < 1e-9, "{fx},{fy}: {zoom:?}");
        }
    }

    #[test]
    fn never_shows_the_edges() {
        for fx in [-50.0, 0.0, 120.0, 400.0, 450.0] {
            for fy in [310.0, 500.0, 800.0, 900.0] {
                let zoom = hold_zoom(fx, fy, 300.0, W, H);
                let left = W / 2.0 - zoom.scale * W / 2.0 + zoom.x;
                let right = W / 2.0 + zoom.scale * W / 2.0 + zoom.x;
                let top = H / 2.0 - zoom.scale * H / 2.0 + zoom.y;
                let bottom = H / 2.0 + zoom.scale * H / 2.0 + zoom.y;
                assert!(left <= 1e-9 && right >= W - 1e-9 && top <= 1e-9 && bottom >= H - 1e-9, "{fx},{fy}");
            }
        }
    }

    #[test]
    fn a_low_bar_zooms_over_a_shorter_way() {
        let high = hold_zoom(200.0, 200.0 + 60.0, 200.0, W, H).scale;
        let low = hold_zoom(200.0, 700.0 + 60.0, 700.0, W, H).scale;
        assert!(low > high);
    }

    #[test]
    fn nonsense_is_no_zoom() {
        assert_eq!(hold_zoom(f64::NAN, 500.0, 300.0, W, H), Zoom::NONE);
        assert_eq!(hold_zoom(200.0, f64::INFINITY, 300.0, W, H), Zoom::NONE);
        assert_eq!(hold_zoom(200.0, 500.0, 300.0, 0.0, H), Zoom::NONE);
    }
}
