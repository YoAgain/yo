//! The held viewer's zoom: a finger holding a bar's media sign and drawn down out of the bar magnifies the photo or
//! video around itself, the further below the bar the more (up to `MOST`, approached softly); moving it then pans,
//! as a magnifying glass would; back at the bar it is the picture as it was. Points (or dp) in screen coordinates.
//! And the recording's zoom (`record_zoom`): a finger filming on a bar and drawn down zooms the camera.

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

/// The recording's zoom: a finger holding a bar to film and drawn out of the bar, down or up, zooms the camera in,
/// growing by the same factor each point, up to `RECORD_MOST` (or the camera's own most, if less) half a screen out;
/// back on the bar it is where it started, on past it the other way it zooms again. Points (or dp) on the screen.
pub const RECORD_MOST: f64 = 10.0;
/// Out of the bar by this much before it starts: a thumb resting on the bar's edge does not zoom.
pub const RECORD_SLACK: f64 = 4.0;

/// The camera's zoom (as its button names it, 1 = the wide lens) for a finger at `finger_y` holding the bar from
/// `bar_top` to `bar_bottom` on a screen `height` high, the zoom having been `start` and the camera going up to
/// `camera_most`. How far out of the bar counts, from its nearer edge: none is `start`, half the screen the most.
pub fn record_zoom(finger_y: f64, bar_top: f64, bar_bottom: f64, height: f64, start: f64, camera_most: f64) -> f64 {
    if !start.is_finite() || start <= 0.0 {
        return 1.0;
    }
    let values = [finger_y, bar_top, bar_bottom, height, camera_most];
    if values.iter().any(|v| !v.is_finite()) || bar_bottom < bar_top || height <= 0.0 {
        return start;
    }
    let out = if finger_y > bar_bottom {
        finger_y - bar_bottom
    } else if finger_y < bar_top {
        bar_top - finger_y
    } else {
        0.0
    };
    let most = RECORD_MOST.min(camera_most);
    let past = out - RECORD_SLACK;
    if past <= 0.0 || most <= start {
        return start;
    }
    // Exponential: each point out multiplies the zoom alike, so it feels even from 1x to the most.
    let travel = (past / (height / 2.0 - RECORD_SLACK).max(1.0)).min(1.0);
    start * (most / start).powf(travel)
}

/// The camera's zoom button's choices, in the wide lens's terms (1 = the wide lens): 1, each lens the camera switches to
/// (`lenses`, e.g. 0.5 for an ultra wide, 3 for a tele), and 2 on the wide one, as the Camera app offers them; only those
/// in `least`..=`most`, a tenth apart at least, smallest first. One choice: the button is not shown.
pub fn camera_choices(least: f64, most: f64, lenses: &[f64]) -> Vec<f64> {
    let mut choices: Vec<f64> = [1.0, 2.0]
        .iter()
        .chain(lenses)
        .map(|choice| (choice * 10.0).round() / 10.0)
        .filter(|choice| choice.is_finite() && *choice >= least - 1e-6 && *choice <= most + 1e-6)
        .collect();
    choices.sort_by(f64::total_cmp);
    choices.dedup_by(|a, b| (*a - *b).abs() < 0.05);
    choices
}

/// The choice after `chosen` (a tap on the button), back to the first after the last.
pub fn camera_next(choices: &[f64], chosen: f64) -> f64 {
    choices.iter().copied().find(|choice| *choice > chosen + 0.001).or(choices.first().copied()).unwrap_or(1.0)
}

/// The button's number: "0.5", "1", "2", "2.5" (no ".0"; the app adds "×").
pub fn camera_label(choice: f64) -> String {
    if choice == choice.round() {
        format!("{}", choice as i64)
    } else {
        format!("{:.1}", choice)
    }
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

    // A bar from 300 to 380 on an 800 high screen: half of it is 400.
    fn rec(y: f64, start: f64, most: f64) -> f64 {
        record_zoom(y, 300.0, 380.0, 800.0, start, most)
    }

    #[test]
    fn recording_on_the_bar_nothing() {
        for y in [300.0, 340.0, 380.0, 380.0 + RECORD_SLACK, 300.0 - RECORD_SLACK] {
            assert_eq!(rec(y, 1.0, 20.0), 1.0, "{y}");
        }
    }

    #[test]
    fn recording_down_grows_evenly_to_the_most_at_half_the_screen() {
        assert!(rec(380.0 + RECORD_SLACK + 1.0, 1.0, 20.0) < 1.02);
        let mut last = 1.0;
        for step in 1..=99 {
            let zoom = rec(380.0 + RECORD_SLACK + step as f64 * 4.0, 1.0, 20.0);
            assert!(zoom > last, "{step}: {zoom}");
            // The same factor each step: exponential.
            if step > 1 {
                let ratio = zoom / last;
                assert!((ratio - RECORD_MOST.powf(4.0 / 396.0)).abs() < 1e-9, "{step}: {ratio}");
            }
            last = zoom;
        }
        assert!((rec(380.0 + 400.0, 1.0, 20.0) - RECORD_MOST).abs() < 1e-9);
        assert_eq!(rec(380.0 + 600.0, 1.0, 20.0), RECORD_MOST);
        // Halfway out, the square root of the way.
        assert!((rec(380.0 + RECORD_SLACK + 198.0, 1.0, 20.0) - RECORD_MOST.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn recording_up_zooms_too() {
        let up = rec(300.0 - RECORD_SLACK - 100.0, 1.0, 20.0);
        let down = rec(380.0 + RECORD_SLACK + 100.0, 1.0, 20.0);
        assert!(up > 1.5 && (up - down).abs() < 1e-9);
        assert!((rec(300.0 - 400.0, 1.0, 20.0) - RECORD_MOST).abs() < 1e-9);
    }

    #[test]
    fn recording_smooth_through_the_edge() {
        // Coming back toward the bar the zoom falls to the start without a jump, from either side.
        for (edge, side) in [(380.0, 1.0), (300.0, -1.0)] {
            let mut last = rec(edge + side * 50.0, 1.0, 20.0);
            for tenth in (0..500).rev() {
                let zoom = rec(edge + side * tenth as f64 / 10.0, 1.0, 20.0);
                assert!(zoom <= last && last - zoom < 0.01, "{tenth}: {last} -> {zoom}");
                last = zoom;
            }
            assert_eq!(last, 1.0);
        }
    }

    #[test]
    fn recording_back_over_the_bar_and_out_the_other_side() {
        assert!(rec(500.0, 1.0, 20.0) > 1.8);
        assert_eq!(rec(340.0, 1.0, 20.0), 1.0);
        assert!(rec(180.0, 1.0, 20.0) > 1.8);
        assert_eq!(rec(340.0, 2.0, 20.0), 2.0);
    }

    #[test]
    fn recording_keeps_to_the_camera() {
        assert!((rec(780.0, 1.0, 3.0) - 3.0).abs() < 1e-9);
        assert!(rec(600.0, 1.0, 3.0) < 3.0);
        assert_eq!(rec(600.0, 12.0, 20.0), 12.0);
        assert_eq!(rec(600.0, 1.0, 1.0), 1.0);
        // From 2x it still reaches the most at half the screen.
        assert!((rec(780.0, 2.0, 20.0) - RECORD_MOST).abs() < 1e-9);
    }

    #[test]
    fn recording_nonsense_is_no_zoom() {
        assert_eq!(rec(f64::NAN, 1.0, 10.0), 1.0);
        assert_eq!(record_zoom(500.0, 300.0, 380.0, f64::INFINITY, 2.0, 10.0), 2.0);
        assert_eq!(record_zoom(500.0, 300.0, 380.0, 0.0, 2.0, 10.0), 2.0);
        assert_eq!(rec(500.0, f64::NAN, 10.0), 1.0);
        assert_eq!(rec(500.0, 1.0, f64::NAN), 1.0);
        assert_eq!(record_zoom(500.0, 380.0, 300.0, 800.0, 1.0, 10.0), 1.0);
    }

    #[test]
    fn camera_choices_are_the_lenses_and_twice_the_wide_one() {
        assert_eq!(camera_choices(0.5, 10.0, &[0.5, 3.0]), vec![0.5, 1.0, 2.0, 3.0]);
        assert_eq!(camera_choices(1.0, 8.0, &[]), vec![1.0, 2.0]);
        assert_eq!(camera_choices(1.0, 1.0, &[]), vec![1.0]);
        assert_eq!(camera_choices(0.6, 8.0, &[0.6, 2.04]), vec![0.6, 1.0, 2.0]);
        assert_eq!(camera_next(&[0.5, 1.0, 2.0], 1.0), 2.0);
        assert_eq!(camera_next(&[0.5, 1.0, 2.0], 2.0), 0.5);
        assert_eq!(camera_next(&[0.5, 1.0, 2.0], 1.4), 2.0);
        assert_eq!(camera_next(&[], 1.0), 1.0);
        assert_eq!(camera_label(0.5), "0.5");
        assert_eq!(camera_label(1.0), "1");
        assert_eq!(camera_label(2.5), "2.5");
    }
}
