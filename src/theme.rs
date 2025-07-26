#[expect(dead_code, clippy::unreadable_literal)]
#[derive(Clone)]
pub(crate) enum Color {
    // // Solarized Dark
    // Foreground = 0x93a1a1,
    // MainIcon = 0xeee8d5,
    // Focused = 0x2aa198,
    // Unfocused = 0x657b83,
    // Good = 0x859900,
    // Notice = 0xb58900,
    // Attention = 0xcb4b16,
    // Critical = 0xdc322f,
    // OKSolar
    Foreground = 0x8faaab,
    MainIcon = 0xf1e9d2,
    Focused = 0x259d94,
    Unfocused = 0x657377,
    Good = 0x819500,
    Notice = 0xac8300,
    Attention = 0xd56500,
    Critical = 0xf23749,
}

pub(crate) const ICON_WARNING: &str = "";

pub(crate) fn ellipsis(s: &str, max_len: Option<usize>) -> String {
    match max_len {
        Some(max_len) => {
            if s.len() > max_len {
                let mut s2: String = s.trim_end().to_owned();
                if s2.len() > max_len {
                    s2.chars()
                        .take(max_len - 1)
                        .collect::<String>()
                        .trim_end()
                        .clone_into(&mut s2);
                    s2.push('…');
                }
                s2
            } else {
                s.to_owned()
            }
        }
        None => s.to_owned(),
    }
}

pub(crate) fn pad(s: &str, min_len: Option<usize>) -> String {
    match min_len {
        Some(min_len) if min_len > s.len() => {
            let pad_count = min_len - s.len();
            format!("{}{}", " ".repeat(pad_count), s)
        }
        _ => s.to_owned(),
    }
}

// Shorten device model name (mouse, headset...)
pub(crate) fn shorten_model_name(s: &str) -> String {
    match s.split(&[' ', '-'][..]).find(|w| {
        w.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && w.chars().any(|c| c.is_ascii_digit())
    }) {
        Some(w) => w.to_owned(),
        None => s
            .split(&[' ', '-'][..])
            .map(|w| {
                if w.chars().all(|c| c.is_ascii_uppercase()) {
                    w.to_owned()
                } else if !w.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    let mut w2 = w.to_owned();
                    w2.truncate(1);
                    w2
                } else {
                    String::new()
                }
            })
            .collect::<String>(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ellipsis() {
        assert_eq!(ellipsis("blah blah blah", None), "blah blah blah");
        assert_eq!(ellipsis("blah blah blah", Some(14)), "blah blah blah");
        assert_eq!(ellipsis("blah blah blah!", Some(14)), "blah blah bla…");
        assert_eq!(ellipsis("blah blah blah ", Some(14)), "blah blah blah");
        assert_eq!(ellipsis("blah blah bla h", Some(14)), "blah blah bla…");
        assert_eq!(ellipsis("blah blah bla ha", Some(15)), "blah blah bla…");
        assert_eq!(ellipsis("éééé", Some(2)), "é…");
    }

    #[test]
    fn test_shorten_model_name() {
        assert_eq!(shorten_model_name("G604 Wireless Gaming Mouse"), "G604");
        assert_eq!(shorten_model_name("Anywhere MX"), "AMX");
        assert_eq!(shorten_model_name("WH-1000XM3"), "WH");
    }
}
