use crate::theme;

pub fn style(
    inner: &str,
    foreground_color: Option<theme::Color>,
    underline_color: Option<theme::Color>,
    overline_color: Option<theme::Color>,
    background_color: Option<theme::Color>,
) -> String {
    let mut r = inner.to_owned();
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

#[allow(dead_code)]
pub enum PolybarActionType {
    ClickLeft = 1,
    ClickMiddle = 2,
    ClickRight = 3,
    ScrollUp = 4,
    ScrollDown = 5,
    DoubleClickLeft = 6,
    DoubleClickMiddle = 7,
    DoubleClickRight = 8,
}

pub struct PolybarAction {
    pub type_: PolybarActionType,
    pub command: String,
}

pub fn action(inner: &str, action: PolybarAction) -> String {
    let cmd = action.command.replace(':', "\\:");
    format!("%{{A{}:{}:}}{}%{{A}}", action.type_ as u8, cmd, inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_style() {
        assert_eq!(
            style("", Some(theme::Color::MainIcon), None, None, None),
            "%{F#eee8d5}%{F-}"
        );
    }

    #[test]
    fn test_action() {
        assert_eq!(
            action(
                ":)",
                PolybarAction {
                    type_: PolybarActionType::ClickRight,
                    command: "this contains a : and ; and \\".to_string()
                }
            ),
            "%{A3:this contains a \\: and ; and \\:}:)%{A}"
        );
    }
}
