use crate::{
    markup,
    polybar_module::{NetworkMode, PolybarModuleEnv, RenderablePolybarModule},
    theme,
};

pub(crate) struct InternetBandwidthModule {
    env: PolybarModuleEnv,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InternetBandwidthModuleState {
    mode: NetworkMode,
}

impl InternetBandwidthModule {
    pub(crate) fn new() -> Self {
        let env = PolybarModuleEnv::new();
        Self { env }
    }
}

const ICON_NETWORK_NORMAL: &str = "󰲝";
const ICON_NETWORK_LOW_BANDWIDTH: &str = "󰅛";

impl RenderablePolybarModule for InternetBandwidthModule {
    type State = InternetBandwidthModuleState;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        if let Some(prev_state) = prev_state {
            let to_wait = match prev_state.mode {
                NetworkMode::Unrestricted => NetworkMode::LowBandwith,
                NetworkMode::LowBandwith => NetworkMode::Unrestricted,
            };
            self.env.wait_network_mode(&to_wait);
        }
    }

    fn update(&mut self) -> Self::State {
        Self::State {
            mode: self.env.network_mode(),
        }
    }

    fn render(&self, state: &Self::State) -> String {
        match state.mode {
            NetworkMode::Unrestricted => markup::action(
                ICON_NETWORK_NORMAL,
                markup::PolybarAction {
                    type_: markup::PolybarActionType::ClickLeft,
                    command: format!("touch {}", self.env.low_bw_filepath.to_str().unwrap()),
                },
            ),
            NetworkMode::LowBandwith => markup::action(
                &markup::style(
                    ICON_NETWORK_LOW_BANDWIDTH,
                    None,
                    Some(theme::Color::Notice),
                    None,
                    None,
                ),
                markup::PolybarAction {
                    type_: markup::PolybarActionType::ClickLeft,
                    command: format!(
                        "rm {}",
                        self.env
                            .low_bw_filepath
                            .as_os_str()
                            .to_os_string()
                            .into_string()
                            .unwrap()
                    ),
                },
            ),
        }
    }
}

#[cfg(test)]
#[expect(clippy::shadow_unrelated)]
mod tests {
    use std::env;

    use super::*;

    #[test]
    fn test_render() {
        let home = env::var("HOME").unwrap();
        let module = InternetBandwidthModule::new();

        let state = InternetBandwidthModuleState {
            mode: NetworkMode::Unrestricted,
        };
        assert_eq!(
            module.render(&state),
            format!("%{{A1:touch {home}/.local/share/low_internet_bandwidth:}}󰲝%{{A}}")
        );

        let state = InternetBandwidthModuleState {
            mode: NetworkMode::LowBandwith,
        };
        assert_eq!(
            module.render(&state),
            format!(
                "%{{A1:rm {home}/.local/share/low_internet_bandwidth:}}%{{u#ac8300}}%{{+u}}󰅛%{{-u}}%{{A}}"
            )
        );
    }
}
