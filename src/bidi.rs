//! Right-to-left text (Arabic, Hebrew, Persian) mixed with English, at word
//! granularity: the Unicode bidi algorithm's ordering applied to whole words.
//! Letters inside a word are joined and ordered by the text shaper.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Ltr,
    Rtl,
    Neutral,
}

fn rtl_char(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// The direction of a word's first letter or digit; punctuation-only words
/// are neutral. Digits read left to right, even Arabic-Indic ones.
fn class(word: &str) -> Class {
    match word.chars().find(|c| c.is_alphanumeric()) {
        Some(c) if rtl_char(c) && c.is_alphabetic() => Class::Rtl,
        Some(_) => Class::Ltr,
        None => Class::Neutral,
    }
}

pub fn is_rtl(word: &str) -> bool {
    class(word) == Class::Rtl
}

/// True when most words are right-to-left. A transcript of mixed speech
/// keeps one direction even when a sentence starts with an English word.
pub fn paragraph_rtl<'a>(words: impl IntoIterator<Item = &'a str>) -> bool {
    let (mut rtl, mut ltr) = (0, 0);
    for w in words {
        match class(w) {
            Class::Rtl => rtl += 1,
            Class::Ltr => ltr += 1,
            Class::Neutral => {}
        }
    }
    rtl > ltr
}

/// Word indices in left-to-right display order. In a right-to-left
/// paragraph the words run right to left, while runs of English words and
/// numbers inside it keep their own left-to-right order.
pub fn visual_order(words: &[&str], rtl: bool) -> Vec<usize> {
    let classes: Vec<Class> = words.iter().map(|w| class(w)).collect();
    let base = if rtl { Class::Rtl } else { Class::Ltr };
    let levels: Vec<u8> = (0..classes.len())
        .map(|i| {
            let dir = match classes[i] {
                // A neutral between words of one direction takes it; otherwise the paragraph's.
                Class::Neutral => {
                    let prev = classes[..i].iter().rev().find(|c| **c != Class::Neutral).copied().unwrap_or(base);
                    let next = classes[i + 1..].iter().find(|c| **c != Class::Neutral).copied().unwrap_or(base);
                    if prev == next { prev } else { base }
                }
                c => c,
            };
            match (rtl, dir) {
                (false, Class::Rtl) | (true, Class::Rtl) => 1,
                (true, _) => 2,
                (false, _) => 0,
            }
        })
        .collect();
    let mut order: Vec<usize> = (0..words.len()).collect();
    let top = levels.iter().copied().max().unwrap_or(0);
    // Reverse every run at or above each level, highest first.
    for level in (1..=top).rev() {
        let mut k = 0;
        while k < order.len() {
            if levels[order[k]] < level {
                k += 1;
                continue;
            }
            let start = k;
            while k < order.len() && levels[order[k]] >= level {
                k += 1;
            }
            order[start..k].reverse();
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visual(line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().collect();
        let rtl = paragraph_rtl(words.iter().copied());
        visual_order(&words, rtl).into_iter().map(|i| words[i]).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn orders_mixed_words() {
        assert_eq!(visual("Welcome to Open Studio."), "Welcome to Open Studio.");
        assert_eq!(visual("أنا عايز أعمل demo سريع"), "سريع demo أعمل عايز أنا");
        // English runs keep their order inside Arabic.
        assert_eq!(visual("أنا بستخدم Open Studio دلوقتي"), "دلوقتي Open Studio بستخدم أنا");
        assert_eq!(visual("الساعة 10 الصبح"), "الصبح 10 الساعة");
        // A short Arabic phrase in an English sentence.
        assert_eq!(visual("I said يلا بينا then left"), "I said بينا يلا then left");
    }
}
