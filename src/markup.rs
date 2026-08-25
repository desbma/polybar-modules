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

/// Icon set rendering a fraction, from empty to full
pub(crate) struct Gauge {
    /// Icon of an exactly empty gauge
    empty: &'static str,
    /// Icons between empty and full, in increasing order of fill
    levels: &'static [&'static str],
    /// Icon of an exactly full gauge
    full: &'static str,
}

impl Gauge {
    /// Build a gauge drawn with circle slices
    pub(crate) const fn circle_slices(empty: &'static str) -> Self {
        Self {
            empty,
            levels: &[
                "󰪞", // nf-md-circle_slice_1
                "󰪟", // nf-md-circle_slice_2
                "󰪠", // nf-md-circle_slice_3
                "󰪡", // nf-md-circle_slice_4
                "󰪢", // nf-md-circle_slice_5
                "󰪣", // nf-md-circle_slice_6
                "󰪤", // nf-md-circle_slice_7
            ],
            full: "󰪥", // nf-md-circle_slice_8
        }
    }

    /// Build a gauge drawn with vertical bars
    pub(crate) const fn ramp() -> Self {
        Self {
            empty: "—",
            levels: &["▁", "▂", "▃", "▄", "▅", "▆", "▇"],
            full: "█",
        }
    }

    /// Render `frac` as the icon whose fill is closest to it
    pub(crate) fn render(&self, frac: f64, color: theme::Color) -> String {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_precision_loss,
            clippy::cast_sign_loss
        )]
        let level = (frac * (self.levels.len() + 1) as f64).round() as usize;
        #[expect(clippy::indexing_slicing)]
        let icon = if frac <= 0.0 {
            self.empty
        } else if frac >= 1.0 {
            self.full
        } else {
            self.levels[level.clamp(1, self.levels.len()) - 1]
        };
        Markup::new(icon).fg(color).into_string()
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
    fn test_ramp() {
        for (frac, expected) in [
            (-0.1, "%{F#819500}—%{F-}"),
            (0.0, "%{F#819500}—%{F-}"),
            (0.0001, "%{F#819500}▁%{F-}"),
            (0.1874, "%{F#819500}▁%{F-}"),
            (0.1875, "%{F#819500}▂%{F-}"),
            (0.3124, "%{F#819500}▂%{F-}"),
            (0.3125, "%{F#819500}▃%{F-}"),
            (0.4374, "%{F#819500}▃%{F-}"),
            (0.4375, "%{F#819500}▄%{F-}"),
            (0.5624, "%{F#819500}▄%{F-}"),
            (0.5625, "%{F#819500}▅%{F-}"),
            (0.6874, "%{F#819500}▅%{F-}"),
            (0.6875, "%{F#819500}▆%{F-}"),
            (0.8124, "%{F#819500}▆%{F-}"),
            (0.8125, "%{F#819500}▇%{F-}"),
            (0.9999, "%{F#819500}▇%{F-}"),
            (1.0, "%{F#819500}█%{F-}"),
            (1.1, "%{F#819500}█%{F-}"),
        ] {
            assert_eq!(Gauge::ramp().render(frac, theme::Color::Good), expected);
        }
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
