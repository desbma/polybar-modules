use std::fmt::Write as _;

use crate::theme;

#[expect(dead_code)]
#[derive(Clone, Copy)]
pub(crate) enum PolybarActionType {
    ClickLeft = 1,
    ClickMiddle = 2,
    ClickRight = 3,
    ScrollUp = 4,
    ScrollDown = 5,
    DoubleClickLeft = 6,
    DoubleClickMiddle = 7,
    DoubleClickRight = 8,
}

enum MarkupOp {
    Foreground(theme::Color),
    Underline(theme::Color),
    Overline(theme::Color),
    Background(theme::Color),
    Font(u8),
    Action {
        type_: PolybarActionType,
        command: String,
    },
}

pub(crate) struct Markup {
    inner: String,
    ops: Vec<MarkupOp>,
}

impl Markup {
    pub(crate) fn new<S>(inner: S) -> Self
    where
        S: Into<String>,
    {
        Self {
            inner: inner.into(),
            ops: Vec::new(),
        }
    }

    pub(crate) fn fg(mut self, color: theme::Color) -> Self {
        self.ops.push(MarkupOp::Foreground(color));
        self
    }

    pub(crate) fn underline(mut self, color: theme::Color) -> Self {
        self.ops.push(MarkupOp::Underline(color));
        self
    }

    #[expect(dead_code)]
    pub(crate) fn overline(mut self, color: theme::Color) -> Self {
        self.ops.push(MarkupOp::Overline(color));
        self
    }

    #[expect(dead_code)]
    pub(crate) fn bg(mut self, color: theme::Color) -> Self {
        self.ops.push(MarkupOp::Background(color));
        self
    }

    #[expect(dead_code)]
    pub(crate) fn font(mut self, index: u8) -> Self {
        self.ops.push(MarkupOp::Font(index));
        self
    }

    pub(crate) fn action<S>(mut self, type_: PolybarActionType, command: S) -> Self
    where
        S: Into<String>,
    {
        self.ops.push(MarkupOp::Action {
            type_,
            command: command.into(),
        });
        self
    }

    pub(crate) fn into_string(self) -> String {
        let mut r = String::new();
        for op in self.ops.iter().rev() {
            match op {
                MarkupOp::Foreground(color) => {
                    let _ = write!(r, "%{{F#{:6x}}}", *color as u32);
                }
                MarkupOp::Underline(color) => {
                    let _ = write!(r, "%{{u#{:06x}}}%{{+u}}", *color as u32);
                }
                MarkupOp::Overline(color) => {
                    let _ = write!(r, "%{{o#{:06x}}}%{{+o}}", *color as u32);
                }
                MarkupOp::Background(color) => {
                    let _ = write!(r, "%{{b#{:6x}}}", *color as u32);
                }
                MarkupOp::Font(index) => {
                    let _ = write!(r, "%{{T{index}}}");
                }
                MarkupOp::Action { type_, command } => {
                    let command_escaped = command.replace(':', "\\:");
                    let _ = write!(r, "%{{A{}:{}:}}", *type_ as u8, command_escaped);
                }
            }
        }

        r.push_str(&self.inner);

        for op in &self.ops {
            match op {
                MarkupOp::Foreground(_) => r.push_str("%{F-}"),
                MarkupOp::Underline(_) => r.push_str("%{-u}"),
                MarkupOp::Overline(_) => r.push_str("%{-o}"),
                MarkupOp::Background(_) => r.push_str("%{b-}"),
                MarkupOp::Font(_) => r.push_str("%{T-}"),
                MarkupOp::Action { .. } => r.push_str("%{A}"),
            }
        }
        r
    }
}

impl From<Markup> for String {
    fn from(val: Markup) -> Self {
        val.into_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markup() {
        assert_eq!(
            Markup::new("").fg(theme::Color::MainIcon).into_string(),
            "%{F#f1e9d2}%{F-}"
        );
    }

    #[test]
    fn test_action() {
        assert_eq!(
            Markup::new(":)")
                .action(
                    PolybarActionType::ClickRight,
                    "this contains a : and ; and \\"
                )
                .into_string(),
            "%{A3:this contains a \\: and ; and \\:}:)%{A}"
        );
    }
}
