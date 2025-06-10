use super::is_systemd_user_unit_running;
use crate::{markup, polybar_module::RenderablePolybarModule, theme};

pub(crate) struct AutolockModule {
    signals: signal_hook::iterator::Signals,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct AutolockModuleState {
    enabled: bool,
}

impl AutolockModule {
    pub(crate) fn new() -> anyhow::Result<Self> {
        let signals = signal_hook::iterator::Signals::new([signal_hook::consts::signal::SIGUSR1])?;
        Ok(Self { signals })
    }
}

const ICON_AUTOLOCK_ENABLED: &str = "󱫗";
const ICON_AUTOLOCK_DISABLED: &str = "󱫕";

impl RenderablePolybarModule for AutolockModule {
    type State = AutolockModuleState;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        if let Some(_prev_state) = prev_state {
            self.signals.forever().next();
        }
    }

    fn update(&mut self) -> Self::State {
        Self::State {
            enabled: is_systemd_user_unit_running("autolock.service"),
        }
    }

    fn render(&self, state: &Self::State) -> String {
        if state.enabled {
            markup::action(
                ICON_AUTOLOCK_ENABLED,
                markup::PolybarAction {
                    type_: markup::PolybarActionType::ClickLeft,
                    command: format!(
                        "systemctl --user stop autolock.service && pkill -USR1 -f '{} autolock$'",
                        env!("CARGO_PKG_NAME")
                    ),
                },
            )
        } else {
            markup::action(
                &markup::style(
                    ICON_AUTOLOCK_DISABLED,
                    None,
                    Some(theme::Color::Notice),
                    None,
                    None,
                ),
                markup::PolybarAction {
                    type_: markup::PolybarActionType::ClickLeft,
                    command: format!(
                        "systemctl --user start autolock.service && pkill -USR1 -f '{} autolock$'",
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
        let module = AutolockModule::new().unwrap();

        let state = AutolockModuleState { enabled: true };
        assert_eq!(
            module.render(&state),
            format!(
                "%{{A1:systemctl --user stop autolock.service && pkill -USR1 -f \'polybar-modules autolock$\':}}󱫗%{{A}}",
            )
        );

        let state = AutolockModuleState { enabled: false };
        assert_eq!(
            module.render(&state),
            format!(
                "%{{A1:systemctl --user start autolock.service && pkill -USR1 -f \'polybar-modules autolock$\':}}%{{u#b58900}}%{{+u}}󱫕%{{-u}}%{{A}}",
            )
        );
    }
}
