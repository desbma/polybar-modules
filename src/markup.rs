use crate::theme;

pub fn style(
    s: &str,
    foreground_color: Option<theme::Color>,
    underline_color: Option<theme::Color>,
    overline_color: Option<theme::Color>,
    background_color: Option<theme::Color>,
) -> String {
    let mut r = s.to_owned();
    if let Some(foreground_color) = foreground_color {
        r = color_markup(r, 'F', foreground_color);
    }
    if let Some(underline_color) = underline_color {
        r = color_markup2(r, 'u', underline_color);
    }
    if let Some(overline_color) = overline_color {
        r = color_markup2(r, 'o', overline_color);
    }
    if let Some(background_color) = background_color {
        r = color_markup(r, 'b', background_color);
    }
    r
}

fn color_markup(s: String, letter: char, color: theme::Color) -> String {
    format!("%{{{}#{:6x}}}{}%{{{}-}}", letter, color as u32, s, letter)
}

fn color_markup2(s: String, letter: char, color: theme::Color) -> String {
    format!(
        "%{{{}#{:06x}}}%{{+{}}}{}%{{-{}}}",
        letter, color as u32, letter, s, letter
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_failed_units() {
        assert_eq!(
            style("", Some(theme::Color::MainIcon), None, None, None),
            "%{F#eee8d5}%{F-}"
        );
    }
}
