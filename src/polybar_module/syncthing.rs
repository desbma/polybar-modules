use std::fs;
use std::thread::sleep;
use std::time::Duration;

use crate::markup;
use crate::polybar_module::RenderablePolybarModule;
use crate::theme;

pub struct SyncthingModule {
    api_key: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SyncthingModuleState {
    folder_count: usize,
    device_connected_count: usize,
    device_syncing_to_count: usize,
    device_syncing_from_count: usize,
    device_total_count: usize,
}

#[derive(serde::Deserialize)]
struct SyncthingLocalConfig {
    gui: SyncthingLocalConfigGui,
}

#[derive(serde::Deserialize)]
struct SyncthingLocalConfigGui {
    apikey: String,
}

impl SyncthingModule {
    pub fn new() -> anyhow::Result<SyncthingModule> {
        let xdg_dirs = xdg::BaseDirectories::with_prefix("syncthing")?;
        let st_config_filepath = xdg_dirs
            .find_config_file("config.xml")
            .ok_or_else(|| anyhow::anyhow!("Unable fo find Synthing config file"))?;
        let st_config_xml = fs::read_to_string(st_config_filepath)?;
        let st_config: SyncthingLocalConfig = quick_xml::de::from_str(&st_config_xml)?;
        Ok(SyncthingModule {
            api_key: st_config.gui.apikey,
        })
    }

    fn try_update(&mut self) -> anyhow::Result<SyncthingModuleState> {
        Ok(SyncthingModuleState {
            folder_count: 0,
            device_connected_count: 0,
            device_syncing_to_count: 0,
            device_syncing_from_count: 0,
            device_total_count: 0,
        })
    }
}

impl RenderablePolybarModule for SyncthingModule {
    type State = Option<SyncthingModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if prev_state.is_some() {
            sleep(Duration::from_secs(1)); // TODO wait for events instead
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
            Some(state) => markup::action(
                &format!(
                    "{}  {}  {}/{} {} {}",
                    markup::style("", Some(theme::Color::MainIcon), None, None, None),
                    state.folder_count,
                    state.device_connected_count,
                    state.device_total_count,
                    state.device_syncing_from_count,
                    state.device_syncing_to_count
                ),
                markup::PolybarAction {
                    type_: markup::PolybarActionType::ClickLeft,
                    command: "firefox --new-tab 'http://127.0.0.1:8384/'".to_string(),
                },
            ),
            None => markup::style("", Some(theme::Color::Attention), None, None, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render() {
        let module = SyncthingModule::new().unwrap();

        let state = Some(SyncthingModuleState {
            folder_count: 1,
            device_connected_count: 2,
            device_syncing_to_count: 3,
            device_syncing_from_count: 4,
            device_total_count: 5,
        });
        assert_eq!(
            module.render(&state),
            "%{A1:firefox --new-tab 'http\\://127.0.0.1\\:8384/':}%{F#eee8d5}%{F-}  1  2/5 4 3%{A}"
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
