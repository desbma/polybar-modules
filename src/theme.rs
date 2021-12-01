#[allow(dead_code)]
#[derive(Clone)]
pub enum Color {
    Foreground = 0x93a1a1,
    MainIcon = 0xeee8d5,
    Focused = 0x2aa198,
    Unfocused = 0x657b83,
    Good = 0x859900,
    Notice = 0xb58900,
    Attention = 0xcb4b16,
    Critical = 0xdc322f,
}

pub fn ellipsis(s: &str, max_len: Option<usize>) -> String {
    match max_len {
        Some(max_len) => {
            if s.len() > max_len {
                let mut s2: String = s.trim_end().to_string();
                if s2.len() > max_len {
                    s2 = s2.chars().take(max_len - 1).collect();
                    s2.push('…');
                }
                s2
            } else {
                s.to_string()
            }
        }
        None => s.to_string(),
    }
}

// Shorten device model name (mouse, headset...)
pub fn shorten_model_name(s: &str) -> String {
    match s.split(&[' ', '-'][..]).find(|w| {
        w.chars()
            .next()
            .map(|c| c.is_ascii_alphabetic())
            .unwrap_or(false)
            && w.chars().any(|c| c.is_ascii_digit())
    }) {
        Some(w) => w.to_string(),
        None => s
            .split(&[' ', '-'][..])
            .map(|w| {
                if w.chars().all(|c| c.is_ascii_uppercase()) {
                    w.to_string()
                } else if !w
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_digit())
                    .unwrap_or(false)
                {
                    let mut w2 = w.to_owned();
                    w2.truncate(1);
                    w2
                } else {
                    "".to_string()
                }
            })
            .collect::<Vec<String>>()
            .join(""),
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
        assert_eq!(ellipsis("éééé", Some(2)), "é…");
    }

    #[test]
    fn test_shorten_model_name() {
        assert_eq!(shorten_model_name("G604 Wireless Gaming Mouse"), "G604");
        assert_eq!(shorten_model_name("Anywhere MX"), "AMX");
        assert_eq!(shorten_model_name("WH-1000XM3"), "WH");
    }
}
