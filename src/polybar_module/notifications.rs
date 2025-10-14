use std::process::Command;

use crate::{markup, polybar_module::RenderablePolybarModule, theme};

pub(crate) struct NotificationsModule {
    signals: signal_hook::iterator::Signals,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct NotificationsModuleState {
    enabled: bool,
}

impl NotificationsModule {
    pub(crate) fn new() -> anyhow::Result<Self> {
        let signals = signal_hook::iterator::Signals::new([signal_hook::consts::signal::SIGUSR1])?;
        Ok(Self { signals })
    }
}

const ICON_NOTIFICATIONS_ENABLED: &str = "󰍩";
const ICON_NOTIFICATIONS_DISABLED: &str = "󰚣";

impl RenderablePolybarModule for NotificationsModule {
    type State = NotificationsModuleState;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        if let Some(_prev_state) = prev_state {
            self.signals.forever().next();
        }
    }

    fn update(&mut self) -> Self::State {
        Self::State {
            enabled: !Command::new("dunstctl")
                .args(["is-paused", "-e"])
                .status()
                .map(|s| s.success())
                .unwrap_or(true),
        }
    }

    fn render(&self, state: &Self::State) -> String {
        if state.enabled {
            markup::action(
                ICON_NOTIFICATIONS_ENABLED,
                markup::PolybarAction {
                    type_: markup::PolybarActionType::ClickLeft,
                    command: format!(
                        "dunstctl set-paused true && pkill -USR1 -f '{} notifications$'",
                        env!("CARGO_PKG_NAME")
                    ),
                },
            )
        } else {
            markup::action(
                &markup::style(
                    ICON_NOTIFICATIONS_DISABLED,
                    None,
                    Some(theme::Color::Notice),
                    None,
                    None,
                ),
                markup::PolybarAction {
                    type_: markup::PolybarActionType::ClickLeft,
                    command: format!(
                        "dunstctl set-paused false && pkill -USR1 -f '{} notifications$'",
                        env!("CARGO_PKG_NAME")
                    ),
                },
            )
        }
    }
}

#[cfg(test)]
#[expect(clippy::shadow_unrelated)]
mod tests {
    use super::*;

    #[test]
    fn test_render() {
        let module = NotificationsModule::new().unwrap();

        let state = NotificationsModuleState { enabled: true };
        assert_eq!(
            module.render(&state),
            "%{A1:dunstctl set-paused true && pkill -USR1 -f 'polybar-modules notifications$':}\u{f0369}%{A}",
        );

        let state = NotificationsModuleState { enabled: false };
        assert_eq!(
            module.render(&state),
            "%{A1:dunstctl set-paused false && pkill -USR1 -f 'polybar-modules notifications$':}%{u#ac8300}%{+u}\u{f06a3}%{-u}%{A}"
        );
    }
}
