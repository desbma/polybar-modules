use std::thread::sleep;
use std::time::Duration;

use crate::markup;
use crate::polybar_module::RenderablePolybarModule;
use crate::theme;

pub struct BluetoothModule {}

#[derive(Debug, PartialEq)]
struct BluetoothDevice {
    connected: bool,
    name: String,
}

#[derive(Debug, PartialEq)]
pub struct BluetoothModuleState {
    controller_powered: bool,
    devices: Vec<BluetoothDevice>,
}

impl BluetoothModule {
    pub fn new() -> anyhow::Result<BluetoothModule> {
        Ok(BluetoothModule {})
    }

    fn try_update(&mut self) -> anyhow::Result<BluetoothModuleState> {
        Ok(BluetoothModuleState {
            controller_powered: false,
            devices: vec![],
        })
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
            Some(state) => {
                let mut fragments: Vec<String> = vec![format!(
                    "{} {}",
                    markup::style("", Some(theme::Color::MainIcon), None, None, None),
                    if state.controller_powered {
                        ""
                    } else {
                        ""
                    },
                )];
                for device in &state.devices {
                    fragments.push(markup::style(
                        &format!(
                            "{}{}",
                            if device.connected { "" } else { "" },
                            device.name
                        ),
                        None,
                        if device.connected {
                            Some(theme::Color::Foreground)
                        } else {
                            None
                        },
                        None,
                        None,
                    ));
                }
                fragments.join(" ")
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

        let state = Some(BluetoothModuleState {
            controller_powered: false,
            devices: vec![],
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} ");

        let state = Some(BluetoothModuleState {
            controller_powered: true,
            devices: vec![],
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} ");

        let state = Some(BluetoothModuleState {
            controller_powered: true,
            devices: vec![
                BluetoothDevice {
                    connected: false,
                    name: "D1".to_string(),
                },
                BluetoothDevice {
                    connected: true,
                    name: "D2".to_string(),
                },
            ],
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-}  D1 %{u#93a1a1}%{+u}D2%{-u}"
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
