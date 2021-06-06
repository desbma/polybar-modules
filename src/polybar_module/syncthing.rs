use std::collections::HashMap;
use std::fs;
use std::thread::sleep;
use std::time::Duration;

use crate::markup;
use crate::polybar_module::RenderablePolybarModule;
use crate::theme;

pub struct SyncthingModule {
    session: reqwest::blocking::Client,
    system_config: Option<SyncthingResponseSystemConfig>,
    last_event_id: u64,
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
struct SyncthingXmlConfig {
    gui: SyncthingXmlConfigGui,
}

#[derive(serde::Deserialize)]
struct SyncthingXmlConfigGui {
    apikey: String,
}

#[derive(serde::Deserialize)]
struct SyncthingResponseSystemConfig {
    folders: Vec<SyncthingResponseSystemConfigFolder>,
    devices: Vec<SyncthingResponseSystemConfigDevice>,
}

#[allow(dead_code)]
#[derive(serde::Deserialize)]
struct SyncthingResponseSystemConfigFolder {
    path: String,
    id: String,
}

#[allow(dead_code)]
#[derive(serde::Deserialize)]
struct SyncthingResponseSystemConfigDevice {
    name: String,
}

#[derive(serde::Deserialize)]
struct SyncthingResponseSystemConnections {
    connections: HashMap<String, SyncthingResponseSystemConnectionsConnection>,
}

#[derive(serde::Deserialize)]
struct SyncthingResponseSystemConnectionsConnection {
    connected: bool,
}

type SyncthingResponseEvents = Vec<SyncthingResponseEventsEvent>;

#[derive(serde::Deserialize)]
struct SyncthingResponseEventsEvent {
    id: u64,
}

const REST_EVENT_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const REST_NORMAL_TIMEOUT: Duration = Duration::from_secs(10);

impl SyncthingModule {
    pub fn new() -> anyhow::Result<SyncthingModule> {
        // Read config to get API key
        let xdg_dirs = xdg::BaseDirectories::with_prefix("syncthing")?;
        let st_config_filepath = xdg_dirs
            .find_config_file("config.xml")
            .ok_or_else(|| anyhow::anyhow!("Unable fo find Synthing config file"))?;
        let st_config_xml = fs::read_to_string(st_config_filepath)?;
        let st_config: SyncthingXmlConfig = quick_xml::de::from_str(&st_config_xml)?;

        // Build session
        let mut session_headers = reqwest::header::HeaderMap::new();
        let mut api_key = reqwest::header::HeaderValue::from_str(&st_config.gui.apikey)?;
        api_key.set_sensitive(true);
        session_headers.insert("X-API-Key", api_key);
        let session = reqwest::blocking::Client::builder()
            .default_headers(session_headers)
            // Set maximum timeout and override with lower one for non event requests otherwise the timeout only
            // applies for connect
            .timeout(REST_EVENT_TIMEOUT)
            .build()?;

        Ok(SyncthingModule {
            session,
            system_config: None,
            last_event_id: 0,
        })
    }

    fn try_update(&mut self) -> anyhow::Result<SyncthingModuleState> {
        let system_config = match &self.system_config {
            None => {
                let system_config_str = self.syncthing_rest_call("system/config")?;
                self.system_config = Some(serde_json::from_str(&system_config_str)?);
                self.system_config.as_ref().unwrap()
            }
            Some(c) => c,
        };

        let system_connections_str = self.syncthing_rest_call("system/connections")?;
        let system_connections: SyncthingResponseSystemConnections =
            serde_json::from_str(&system_connections_str)?;

        Ok(SyncthingModuleState {
            folder_count: system_config.folders.len(),
            device_connected_count: system_connections
                .connections
                .values()
                .filter(|c| c.connected)
                .count(),
            device_syncing_to_count: 0,
            device_syncing_from_count: 0,
            device_total_count: system_config.devices.len(),
        })
    }

    fn syncthing_events(&self) -> anyhow::Result<SyncthingResponseEvents> {
        // See https://docs.syncthing.net/dev/events.html
        let mut url = reqwest::Url::parse("http://127.0.0.1:8384/rest/events")?;
        url.query_pairs_mut()
            .append_pair("since", &self.last_event_id.to_string());
        url.query_pairs_mut().append_pair(
            "timeout",
            &(REST_EVENT_TIMEOUT + REST_NORMAL_TIMEOUT)
                .as_secs()
                .to_string(),
        );
        log::debug!("GET {:?}", url.to_string());
        let json_str = self.session.get(url).send()?.text()?;
        log::trace!("{}", json_str);
        let events: SyncthingResponseEvents = serde_json::from_str(&json_str)?;
        Ok(events)
    }

    fn syncthing_rest_call(&self, path: &str) -> anyhow::Result<String> {
        let url = format!("http://127.0.0.1:8384/rest/{}", path);
        log::debug!("GET {:?}", url);
        let json_str = self
            .session
            .get(url)
            .timeout(REST_NORMAL_TIMEOUT)
            .send()?
            .text()?;
        log::trace!("{}", json_str);
        Ok(json_str)
    }
}

impl RenderablePolybarModule for SyncthingModule {
    type State = Option<SyncthingModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if prev_state.is_some() {
            loop {
                if let Ok(events) = self.syncthing_events() {
                    // TODO filter events to wait for
                    if let Some(last_evt) = events.iter().last() {
                        self.last_event_id = last_evt.id;
                    } else {
                        continue;
                    }
                } else {
                    sleep(Duration::from_secs(10));
                }
                break;
            }
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
