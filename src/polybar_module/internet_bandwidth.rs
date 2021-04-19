use crate::markup;
use crate::polybar_module::{PolybarModuleEnv, RenderablePolybarModule, RuntimeMode};
use crate::theme;

pub struct InternetBandwidthModule {
    env: PolybarModuleEnv,
    cur_state: InternetBandwidthModuleState,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InternetBandwidthModuleState {
    mode: RuntimeMode,
}

impl InternetBandwidthModule {
    pub fn new() -> InternetBandwidthModule {
        let env = PolybarModuleEnv::new();
        let cur_state = InternetBandwidthModuleState {
            mode: env.get_runtime_mode(),
        };
        InternetBandwidthModule { env, cur_state }
    }
}

impl RenderablePolybarModule for InternetBandwidthModule {
    type State = InternetBandwidthModuleState;

    fn wait_update(&mut self, first_update: bool) {
        if !first_update {
            let to_wait = match self.cur_state.mode {
                RuntimeMode::Unrestricted => RuntimeMode::LowNetworkBandwith,
                RuntimeMode::LowNetworkBandwith => RuntimeMode::Unrestricted,
            };
            self.env.wait_runtime_mode(to_wait);
        }
    }

    fn update(&mut self) -> Self::State {
        self.cur_state = Self::State {
            mode: self.env.get_runtime_mode(),
        };
        self.cur_state.clone()
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

    #[test]
    fn test_render() {
        let home = std::env::var("HOME").unwrap();
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
