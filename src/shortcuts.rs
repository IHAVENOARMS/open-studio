//! Keyboard shortcuts found in a recording's key events, for the on-video
//! keycap overlay (Screen Studio's "Keyboard shortcuts" tab).

use crate::project::Recording;

#[derive(Clone, Debug)]
pub struct Shortcut {
    /// Stable id: index of the key-down event.
    pub id: usize,
    pub t: f64,
    pub keys: Vec<&'static str>,
}

impl Shortcut {
    pub fn label(&self) -> String {
        self.keys.join(" + ")
    }
}

fn modifier(code: u16) -> Option<usize> {
    match code {
        29 | 97 => Some(0),   // Ctrl
        56 | 100 => Some(1),  // Alt
        42 | 54 => Some(2),   // Shift
        125 | 126 => Some(3), // Super
        _ => None,
    }
}

const MOD_NAMES: [&str; 4] = ["Ctrl", "Alt", "Shift", "Super"];

/// Display name for an evdev key code.
pub fn key_name(code: u16) -> Option<&'static str> {
    const LETTERS: [(u16, &str); 26] = [
        (30, "A"), (48, "B"), (46, "C"), (32, "D"), (18, "E"), (33, "F"), (34, "G"), (35, "H"), (23, "I"), (36, "J"), (37, "K"),
        (38, "L"), (50, "M"), (49, "N"), (24, "O"), (25, "P"), (16, "Q"), (19, "R"), (31, "S"), (20, "T"), (22, "U"), (47, "V"),
        (17, "W"), (45, "X"), (21, "Y"), (44, "Z"),
    ];
    if let Some((_, n)) = LETTERS.iter().find(|(c, _)| *c == code) {
        return Some(n);
    }
    Some(match code {
        2 => "1", 3 => "2", 4 => "3", 5 => "4", 6 => "5", 7 => "6", 8 => "7", 9 => "8", 10 => "9", 11 => "0",
        1 => "Esc", 14 => "Backspace", 15 => "Tab", 28 => "Enter", 57 => "Space", 111 => "Delete", 110 => "Insert",
        102 => "Home", 107 => "End", 104 => "Page Up", 109 => "Page Down",
        103 => "↑", 108 => "↓", 105 => "←", 106 => "→",
        12 => "-", 13 => "=", 26 => "[", 27 => "]", 39 => ";", 40 => "'", 41 => "`", 43 => "\\", 51 => ",", 52 => ".", 53 => "/",
        59 => "F1", 60 => "F2", 61 => "F3", 62 => "F4", 63 => "F5", 64 => "F6", 65 => "F7", 66 => "F8", 67 => "F9", 68 => "F10",
        87 => "F11", 88 => "F12",
        _ => return None,
    })
}

/// A combination as typed in settings ("shift + tab", "Ctrl+Enter") in the
/// overlay's own form: modifiers in Ctrl, Alt, Shift, Super order, then the
/// key, joined by "+".
pub fn normalize(combo: &str) -> String {
    let mut mods = [false; 4];
    let mut key = String::new();
    for part in combo.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => mods[0] = true,
            "alt" | "option" => mods[1] = true,
            "shift" => mods[2] = true,
            "super" | "meta" | "win" | "windows" | "cmd" | "logo" => mods[3] = true,
            "esc" | "escape" => key = "Esc".into(),
            "return" | "enter" => key = "Enter".into(),
            "up" | "arrowup" => key = "↑".into(),
            "down" | "arrowdown" => key = "↓".into(),
            "left" | "arrowleft" => key = "←".into(),
            "right" | "arrowright" => key = "→".into(),
            "pageup" | "page up" => key = "Page Up".into(),
            "pagedown" | "page down" => key = "Page Down".into(),
            "del" | "delete" => key = "Delete".into(),
            "space" => key = "Space".into(),
            "tab" => key = "Tab".into(),
            "backspace" => key = "Backspace".into(),
            "home" => key = "Home".into(),
            "end" => key = "End".into(),
            "insert" => key = "Insert".into(),
            "minus" => key = "-".into(),
            "equals" | "plus" => key = "=".into(),
            other if other.len() == 1 => key = other.to_uppercase(),
            other if other.starts_with('f') && other[1..].parse::<u8>().is_ok() => key = other.to_uppercase(),
            _ => key = part.to_string(),
        }
    }
    let mut out: Vec<String> = (0..4).filter(|m| mods[*m]).map(|m| MOD_NAMES[m].to_string()).collect();
    if !key.is_empty() {
        out.push(key);
    }
    out.join("+")
}

/// Shortcuts in key-press order. Combinations need Ctrl, Alt or Super (Shift
/// alone is typing). With `single_keys`, isolated single presses also count,
/// but not fast typing runs. `custom` combinations always count.
pub fn detect(rec: &Recording, single_keys: bool, custom: &[String]) -> Vec<Shortcut> {
    let custom: Vec<String> = custom.iter().map(|c| normalize(c)).collect();
    let keys = &rec.events.keys;
    let mut held = [false; 4];
    let mut out = vec![];
    for (i, k) in keys.iter().enumerate() {
        if let Some(m) = modifier(k.code) {
            held[m] = k.down;
            continue;
        }
        if !k.down {
            continue;
        }
        let Some(name) = key_name(k.code) else { continue };
        let pressed: Vec<&'static str> = (0..4).filter(|m| held[*m]).map(|m| MOD_NAMES[m]).chain([name]).collect();
        let combo = held[0] || held[1] || held[3];
        if custom.contains(&pressed.join("+")) {
            out.push(Shortcut { id: i, t: k.t, keys: pressed });
        } else if combo {
            let mut names: Vec<&'static str> = (0..4).filter(|m| held[*m]).map(|m| MOD_NAMES[m]).collect();
            names.push(name);
            out.push(Shortcut { id: i, t: k.t, keys: names });
        } else if single_keys {
            // Skip typing: another key within 0.35s on either side.
            let typing = keys.iter().enumerate().any(|(j, o)| {
                j != i && o.down && modifier(o.code).is_none() && (o.t - k.t).abs() < 0.35
            });
            if !typing {
                let mut names = vec![];
                if held[2] {
                    names.push("Shift");
                }
                names.push(name);
                out.push(Shortcut { id: i, t: k.t, keys: names });
            }
        }
    }
    out
}

#[cfg(test)]
mod custom_tests {
    use super::*;

    #[test]
    fn custom_combinations_always_show() {
        assert_eq!(normalize("shift + tab"), "Shift+Tab");
        assert_eq!(normalize("super+ctrl+up"), "Ctrl+Super+↑");
        assert_eq!(normalize("Return"), "Enter");
        let key = |t: f64, code: u16, down: bool| crate::project::Key { t, code, down };
        let mut rec = crate::project::Recording {
            version: 1,
            video: "screen.mp4".into(),
            width: 1920,
            height: 1080,
            fps: 60,
            duration: 5.0,
            cursor_embedded: false,
            source: Default::default(),
            events: Default::default(),
            custom_cursors: vec![],
            audio: vec![],
            camera: None,
            dir: Default::default(),
            transcript: None,
        };
        // Enter alone, then Shift+Tab: typing-like, so hidden by default.
        rec.events.keys = vec![key(1.0, 28, true), key(1.1, 28, false), key(3.0, 42, true), key(3.05, 15, true), key(3.1, 15, false), key(3.2, 42, false)];
        assert!(detect(&rec, false, &[]).is_empty());
        let found = detect(&rec, false, &["Enter".into(), "shift+tab".into()]);
        assert_eq!(found.iter().map(|s| s.label()).collect::<Vec<_>>(), vec!["Enter", "Shift + Tab"]);
    }
}
