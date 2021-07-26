use std::thread::sleep;
use std::time::Duration;

use crate::markup;
use crate::polybar_module::RenderablePolybarModule;
use crate::theme;

pub struct BluetoothModule {}

#[derive(Debug, PartialEq)]
pub struct BluetoothModuleState {}

impl BluetoothModule {
    pub fn new() -> anyhow::Result<BluetoothModule> {
        Ok(BluetoothModule {})
    }

    fn try_update(&mut self) -> anyhow::Result<BluetoothModuleState> {
        Ok(BluetoothModuleState {})
    }
}

impl RenderablePolybarModule for BluetoothModule {
    type State = Option<BluetoothModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if prev_state.is_some() {
            sleep(Duration::from_secs(1));
        }
    }

    fn update(&mut self) -> Self::State {
        match self.try_update() {
            Ok(s) => Some(s),
            Err(e) => {
                log::error!("{}", e);
                None
            }
        }
    }

    fn render(&self, state: &Self::State) -> String {
        match state {
            Some(_state) => {
                format!(
                    "{} ",
                    markup::style("", Some(theme::Color::MainIcon), None, None, None),
                )
            }
            None => markup::style("", Some(theme::Color::Attention), None, None, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render() {
        let module = BluetoothModule::new().unwrap();

        let state = Some(BluetoothModuleState {});
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} ");

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
