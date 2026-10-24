//! J/K/L shuttle, as in Screen Studio and other editors: L plays forwards
//! and each further press doubles the speed, J does the same backwards, and
//! K plays and pauses. Speeds go up to 16x either way.

/// Fastest shuttle speed, either way.
pub const MAX: f64 = 16.0;

/// Speeds offered in the playback speed menu.
pub const MENU: [f64; 5] = [1.0, 2.0, 4.0, 8.0, 16.0];

/// What happens to the playback rate (signed; 0 is paused) on a key press.
pub fn press_l(rate: f64) -> f64 {
    if rate <= 0.0 { 1.0 } else { (rate * 2.0).min(MAX) }
}

pub fn press_j(rate: f64) -> f64 {
    if rate >= 0.0 { -1.0 } else { (rate * 2.0).max(-MAX) }
}

pub fn press_k(rate: f64) -> f64 {
    if rate == 0.0 { 1.0 } else { 0.0 }
}

/// Label for a rate: "2×", "-4×".
pub fn label(rate: f64) -> String {
    let n = if rate.fract() == 0.0 { format!("{}", rate.abs()) } else { format!("{:.1}", rate.abs()) };
    if rate < 0.0 { format!("-{n}×") } else { format!("{n}×") }
}

/// Plays sound at this rate? Speech past 2x is noise, and backwards too.
pub fn audible(rate: f64) -> bool {
    rate > 0.0 && rate <= 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shuttle_doubles_up_to_16x_and_turns_around() {
        let mut r = 0.0;
        let mut seen = vec![];
        for _ in 0..6 {
            r = press_l(r);
            seen.push(r);
        }
        assert_eq!(seen, [1.0, 2.0, 4.0, 8.0, 16.0, 16.0]);
        // J while going forwards starts backwards at normal speed.
        r = press_j(r);
        assert_eq!(r, -1.0);
        r = press_j(press_j(r));
        assert_eq!(r, -4.0);
        assert_eq!(press_j(press_j(press_j(r))), -16.0);
        // L while going backwards plays forwards again.
        assert_eq!(press_l(r), 1.0);
        // K pauses whatever the speed, and plays from paused.
        assert_eq!(press_k(-4.0), 0.0);
        assert_eq!(press_k(16.0), 0.0);
        assert_eq!(press_k(0.0), 1.0);
        assert_eq!(label(-4.0), "-4×");
        assert_eq!(label(16.0), "16×");
        assert!(audible(2.0) && !audible(4.0) && !audible(-1.0));
    }
}
