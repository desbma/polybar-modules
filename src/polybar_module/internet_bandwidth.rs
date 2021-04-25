use crate::markup;
use crate::polybar_module::{PolybarModuleEnv, RenderablePolybarModule, RuntimeMode};
use crate::theme;

pub struct InternetBandwidthModule {
    env: PolybarModuleEnv,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InternetBandwidthModuleState {
    mode: RuntimeMode,
}

impl InternetBandwidthModule {
    pub fn new() -> InternetBandwidthModule {
        let env = PolybarModuleEnv::new();
        InternetBandwidthModule { env }
    }
}

impl RenderablePolybarModule for InternetBandwidthModule {
    type State = InternetBandwidthModuleState;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if let Some(prev_state) = prev_state {
            let to_wait = match prev_state.mode {
                RuntimeMode::Unrestricted => RuntimeMode::LowNetworkBandwith,
                RuntimeMode::LowNetworkBandwith => RuntimeMode::Unrestricted,
            };
            self.env.wait_runtime_mode(to_wait);
        }
    }

    fn update(&mut self) -> Self::State {
        Self::State {
            mode: self.env.get_runtime_mode(),
        }
    }

    fn render(&self, state: &Self::State) -> String {
        match state.mode {
            RuntimeMode::Unrestricted => markup::action(
                "",
                markup::PolybarAction {
                    type_: markup::PolybarActionType::ClickLeft,
                    command: format!(
                        "touch {}",
                        self.env
                            .low_bw_filepath
                            .as_os_str()
                            .to_os_string()
                            .into_string()
                            .unwrap()
                    ),
                },
            ),
            RuntimeMode::LowNetworkBandwith => markup::action(
                &markup::style("", None, Some(theme::Color::Notice), None, None),
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
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_render() {
        let home = env::var("HOME").unwrap();
        let module = InternetBandwidthModule::new();

        let state = InternetBandwidthModuleState {
            mode: RuntimeMode::Unrestricted,
        };
        assert_eq!(
            module.render(&state),
            format!(
                "%{{A1:touch {}/.local/share/low_internet_bandwidth:}}%{{A}}",
                home
            )
        );

        let state = InternetBandwidthModuleState {
            mode: RuntimeMode::LowNetworkBandwith,
        };
        assert_eq!(
            module.render(&state),
            format!(
                "%{{A1:rm {}/.local/share/low_internet_bandwidth:}}%{{u#b58900}}%{{+u}}%{{-u}}%{{A}}",
                home
            )
        );
    }
}
