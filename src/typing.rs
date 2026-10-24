//! Speed up typing, as in Screen Studio: stretches of the recording where
//! you're typing are found from the recorded keys and offered as amber
//! "Typing" suggestions on the timeline. Applying one speeds that part up
//! (a slice with its own speed); removing one hides it for good.

use crate::edit::EditConfig;
use crate::project::Key;

/// Suggested speed, and the slider's range.
pub const DEFAULT_SPEED: f64 = 4.0;
pub const MIN_SPEED: f64 = 1.0;
pub const MAX_SPEED: f64 = 8.0;

/// Pauses longer than this end a typing stretch (thinking, reading).
const MAX_GAP: f64 = 1.5;
/// A stretch needs this many keystrokes and seconds to be worth speeding up.
const MIN_KEYS: usize = 8;
const MIN_SPAN: f64 = 1.5;
/// Kept around the first and last keystroke.
const LEAD: f64 = 0.15;
const TAIL: f64 = 0.3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Typing {
    pub start: f64,
    pub end: f64,
    pub keys: usize,
}

/// Keys that type something: letters, digits, punctuation, space, Tab,
/// Enter and Backspace (evdev codes).
fn types(code: u16) -> bool {
    matches!(code, 2..=28 | 30..=41 | 43..=53 | 57)
}

/// Typing stretches in recording `keys`, in order. Shortcuts (Ctrl, Alt or
/// Super held) don't count as typing; Shift does.
pub fn detect(keys: &[Key], duration: f64) -> Vec<Typing> {
    let (mut ctrl, mut alt, mut sup) = (false, false, false);
    let mut presses = vec![];
    for k in keys {
        match k.code {
            29 | 97 => ctrl = k.down,
            56 | 100 => alt = k.down,
            125 | 126 => sup = k.down,
            c if k.down && types(c) && !(ctrl || alt || sup) => presses.push(k.t),
            _ => {}
        }
    }
    let mut out = vec![];
    let mut i = 0;
    while i < presses.len() {
        let mut j = i;
        while j + 1 < presses.len() && presses[j + 1] - presses[j] <= MAX_GAP {
            j += 1;
        }
        let (a, b) = (presses[i], presses[j]);
        if j + 1 - i >= MIN_KEYS && b - a >= MIN_SPAN {
            out.push(Typing { start: (a - LEAD).max(0.0), end: (b + TAIL).min(duration), keys: j + 1 - i });
        }
        i = j + 1;
    }
    out
}

/// Suggestions still to show: not removed, not sped up already, and mostly
/// in the kept video.
pub fn suggestions(found: &[Typing], edit: &EditConfig, duration: f64) -> Vec<Typing> {
    let kept = edit.kept_ranges(duration);
    found
        .iter()
        .copied()
        .filter(|t| !edit.typing_removed.iter().any(|r| (r - t.start).abs() < 0.01))
        .filter(|t| edit.slice_props((t.start + t.end) * 0.5).speed == 1.0)
        .filter(|t| {
            let inside: f64 = kept.iter().map(|&(a, b)| (b.min(t.end) - a.max(t.start)).max(0.0)).sum();
            inside > (t.end - t.start) * 0.5
        })
        .collect()
}

/// Speeds `t` up: its own slice, at `speed`.
pub fn apply(edit: &mut EditConfig, t: Typing, speed: f64, duration: f64) {
    edit.split_at(t.start, duration);
    edit.split_at(t.end, duration);
    edit.set_slice_props(t.start, t.end, |p| p.speed = speed);
}

/// Hides `t`'s suggestion.
pub fn remove(edit: &mut EditConfig, t: Typing) {
    edit.typing_removed.push(t.start);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(t: f64, code: u16, down: bool) -> Key {
        Key { t, code, down }
    }

    /// Letters every `dt` seconds from `from`, `n` of them.
    fn typed(from: f64, n: usize, dt: f64) -> Vec<Key> {
        (0..n).flat_map(|i| [key(from + i as f64 * dt, 30, true), key(from + i as f64 * dt + 0.05, 30, false)]).collect()
    }

    #[test]
    fn finds_typing_not_shortcuts_or_stray_keys() {
        let mut keys = typed(2.0, 20, 0.15); // 2.0..4.85
        keys.extend(typed(5.5, 10, 0.2)); // within 1.5s: same stretch, to 7.3
        keys.extend(typed(12.0, 3, 0.2)); // too few keys
        // Ctrl held: shortcuts, not typing.
        keys.push(key(20.0, 29, true));
        keys.extend(typed(20.1, 12, 0.15));
        keys.push(key(22.0, 29, false));
        // Shift is still typing.
        keys.push(key(30.0, 42, true));
        keys.extend(typed(30.0, 10, 0.2));
        keys.push(key(32.0, 42, false));
        let found = detect(&keys, 40.0);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!((found[0].start - 1.85).abs() < 1e-9 && (found[0].end - 7.6).abs() < 1e-9 && found[0].keys == 30);
        assert!((found[1].start - 29.85).abs() < 1e-9);
        // Long pauses split stretches; the end is clamped to the recording.
        let found = detect(&typed(0.0, 10, 0.2).into_iter().chain(typed(5.0, 10, 0.2)).collect::<Vec<_>>(), 6.0);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].start, found[1].end), (0.0, 6.0));
    }

    #[test]
    fn apply_remove_and_undo() {
        let duration = 30.0;
        let found = detect(&typed(2.0, 20, 0.15).into_iter().chain(typed(10.0, 20, 0.15)).collect::<Vec<_>>(), duration);
        assert_eq!(found.len(), 2);
        let mut edit = EditConfig::default();
        let before = edit.clone();
        assert_eq!(suggestions(&found, &edit, duration).len(), 2);
        // Applying speeds up exactly that stretch, as its own slice.
        apply(&mut edit, found[0], 4.0, duration);
        let t = found[0];
        assert_eq!(edit.slice_props((t.start + t.end) / 2.0).speed, 4.0);
        assert_eq!(edit.slice_props(t.start - 0.01).speed, 1.0);
        assert_eq!(edit.slice_props(t.end + 0.01).speed, 1.0);
        assert!(edit.slice_ranges(duration).iter().any(|&(a, b)| (a - t.start).abs() < 1e-9 && (b - t.end).abs() < 1e-9));
        assert_eq!(suggestions(&found, &edit, duration), vec![found[1]]);
        // The edited video gets shorter by the time saved.
        let len = |e: &EditConfig| e.segments(duration).iter().map(|(a, b, s)| (b - a) / s).sum::<f64>();
        assert!((len(&before) - len(&edit) - (t.end - t.start) * 0.75).abs() < 1e-6);
        // Removing hides the other one.
        remove(&mut edit, found[1]);
        assert!(suggestions(&found, &edit, duration).is_empty());
        // Undo (the old config) brings both back.
        assert_eq!(suggestions(&found, &before, duration).len(), 2);
        // Cut away: no suggestion.
        let mut cut = before.clone();
        cut.add_cut(found[1].start - 1.0, found[1].end + 1.0);
        assert_eq!(suggestions(&found, &cut, duration), vec![found[0]]);
        // Fast speeds reach the export.
        let mut fast = before.clone();
        apply(&mut fast, found[0], MAX_SPEED, duration);
        assert!(fast.segments(duration).iter().any(|s| s.2 == MAX_SPEED));
    }
}
