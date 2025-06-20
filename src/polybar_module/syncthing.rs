use std::{cmp::max, collections::HashSet, fs, io, path::Path, thread::sleep, time::Duration};

use crate::{
    markup,
    polybar_module::{RenderablePolybarModule, TCP_LOCAL_TIMEOUT, syncthing_rest},
    theme::{self, ICON_WARNING},
};

pub(crate) struct SyncthingModule {
    session: ureq::Agent,
    api_key: String,
    system_config: Option<syncthing_rest::SystemConfig>,
    last_event_id: u64,
    folders_syncing_down: HashSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(clippy::struct_field_names)]
pub(crate) struct SyncthingModuleState {
    folder_count: usize,
    device_connected_count: usize,
    device_syncing_to_count: usize,
    folders_syncing_down_count: usize,
    remote_device_count: usize,
}

#[derive(serde::Deserialize)]
struct SyncthingXmlConfig {
    gui: SyncthingXmlConfigGui,
}

#[derive(serde::Deserialize)]
struct SyncthingXmlConfigGui {
    apikey: String,
}

#[derive(thiserror::Error, Debug)]
enum HttpError {
    #[error("HTTP status code {0}: {1}")]
    Status(u16, String),
    #[error("URL error: {0}")]
    Url(#[from] url::ParseError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("HTTP error {0}")]
    Other(#[from] Box<ureq::Error>),
}

const REST_EVENT_TIMEOUT: Duration = Duration::from_secs(60 * 60);

impl SyncthingModule {
    pub(crate) fn new(st_config_filepath: &Path) -> anyhow::Result<Self> {
        // Read config to get API key
        log::debug!("st_config_filepath = {st_config_filepath:?}");
        let st_config_xml = fs::read_to_string(st_config_filepath)?;
        let st_config: SyncthingXmlConfig = quick_xml::de::from_str(&st_config_xml)?;

        // Build session
        let session = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                // Set maximum timeout and override with lower one for non event requests otherwise the timeout only
                // applies for connect
                .timeout_global(Some(max(TCP_LOCAL_TIMEOUT, REST_EVENT_TIMEOUT)))
                .build(),
        );

        Ok(Self {
            api_key: st_config.gui.apikey,
            session,
            system_config: None,
            last_event_id: 0,
            folders_syncing_down: HashSet::new(),
        })
    }

    fn try_update(&mut self) -> anyhow::Result<SyncthingModuleState> {
        let system_config = match &self.system_config {
            None => {
                let system_config_str = self.syncthing_rest_call("system/config", &[])?;
                self.system_config = Some(serde_json::from_str(&system_config_str)?);
                self.system_config.as_ref().unwrap()
            }
            Some(c) => c,
        };

        let system_connections_str = self.syncthing_rest_call("system/connections", &[])?;
        let system_connections: syncthing_rest::SystemConnections =
            serde_json::from_str(&system_connections_str)?;

        let mut device_syncing_to_count = 0;
        for (device_id, device) in &system_connections.connections {
            if device.connected {
                let db_completion_str =
                    match self.syncthing_rest_call("db/completion", &[("device", device_id)]) {
                        Ok(s) => s,
                        Err(HttpError::Status(404, _)) => {
                            // Paused devices return 404
                            continue;
                        }
                        Err(e) => return Err(e.into()),
                    };
                let db_completion: syncthing_rest::DbCompletion =
                    serde_json::from_str(&db_completion_str)?;
                if (db_completion.need_bytes > 0)
                    || (db_completion.need_items > 0)
                    || (db_completion.need_deletes > 0)
                {
                    device_syncing_to_count += 1;
                }
            }
        }

        Ok(SyncthingModuleState {
            folder_count: system_config.folders.len(),
            device_connected_count: system_connections
                .connections
                .values()
                .filter(|c| c.connected)
                .count(),
            device_syncing_to_count,
            folders_syncing_down_count: self.folders_syncing_down.len(),
            remote_device_count: system_config.devices.len() - 1, // -1 to account for local device
        })
    }

    fn syncthing_events(&self, evt_types: &[&str]) -> anyhow::Result<Vec<syncthing_rest::Event>> {
        // See https://docs.syncthing.net/dev/events.html
        let mut url = url::Url::parse("http://127.0.0.1:8384/rest/events")?;
        url.query_pairs_mut()
            .append_pair("since", &self.last_event_id.to_string())
            .append_pair("events", &evt_types.join(","));
        url.query_pairs_mut().append_pair(
            "timeout",
            &(REST_EVENT_TIMEOUT + TCP_LOCAL_TIMEOUT)
                .as_secs()
                .to_string(),
        );
        log::debug!("GET {:?}", url.as_str());
        let response = self
            .session
            .get(url.as_str())
            .header("X-API-Key", &self.api_key)
            .call()?;
        anyhow::ensure!(
            response.status().is_success(),
            "HTTP response {}",
            response.status()
        );
        let json_str = response.into_body().read_to_string()?;
        log::trace!("{json_str}");
        let events: Vec<syncthing_rest::Event> = serde_json::from_str(&json_str)?;
        Ok(events)
    }

    fn syncthing_rest_call(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<String, HttpError> {
        let base_url = url::Url::parse("http://127.0.0.1:8384/rest/")?;
        let mut url = base_url.join(path)?;
        for (param_key, param_val) in params {
            url.query_pairs_mut().append_pair(param_key, param_val);
        }
        log::debug!("GET {:?}", url.as_str());
        let response = self
            .session
            .get(url.as_str())
            .header("X-API-Key", &self.api_key)
            .config()
            .timeout_global(Some(TCP_LOCAL_TIMEOUT))
            .build()
            .call()
            .map_err(Box::new)?;
        if !response.status().is_success() {
            return Err(HttpError::Status(
                response.status().as_u16(),
                response
                    .status()
                    .canonical_reason()
                    .unwrap_or("?")
                    .to_owned(),
            ));
        }
        let json_str = response.into_body().read_to_string().map_err(Box::new)?;
        log::trace!("{json_str}");
        Ok(json_str)
    }
}

const ICON_SYNCTHING: &str = "󱋖";
const ICON_SYNCTHING_FOLDER: &str = "";
const ICON_SYNCTHING_DEVICE: &str = "";
const ICON_SYNCTHING_UPLOADING: &str = "";
const ICON_SYNCTHING_DOWNLOADING: &str = "";

#[expect(clippy::single_match)]
impl RenderablePolybarModule for SyncthingModule {
    type State = Option<SyncthingModuleState>;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        if prev_state.is_some() {
            if let Ok(events) = self.syncthing_events(&[
                "DeviceConnected",
                "DeviceDisconnected",
                "DownloadProgress",
                "RemoteDownloadProgress",
            ]) {
                for event in events {
                    log::debug!("{event:?}");
                    match event.data {
                        syncthing_rest::EventData::DownloadProgress(event_data) => {
                            self.folders_syncing_down.clear();
                            for folder in event_data.keys() {
                                self.folders_syncing_down.insert(folder.to_owned());
                            }
                        }
                        _ => {}
                    }
                    self.last_event_id = event.id;
                }
            } else {
                sleep(Duration::from_secs(10));
            }
        }
    }

    fn update(&mut self) -> Self::State {
        match self.try_update() {
            Ok(s) => Some(s),
            Err(e) => {
                log::error!("{e}");
                None
            }
        }
    }

    fn render(&self, state: &Self::State) -> String {
        match state {
            Some(state) => markup::action(
                &format!(
                    "{} {} {} {} {}/{} {}{} {}{}",
                    markup::style(
                        ICON_SYNCTHING,
                        Some(theme::Color::MainIcon),
                        None,
                        None,
                        None
                    ),
                    ICON_SYNCTHING_FOLDER,
                    state.folder_count,
                    ICON_SYNCTHING_DEVICE,
                    state.device_connected_count,
                    state.remote_device_count,
                    ICON_SYNCTHING_DOWNLOADING,
                    state.folders_syncing_down_count,
                    ICON_SYNCTHING_UPLOADING,
                    state.device_syncing_to_count
                ),
                markup::PolybarAction {
                    type_: markup::PolybarActionType::ClickLeft,
                    command: "firefox --new-tab 'http://127.0.0.1:8384/'".to_owned(),
                },
            ),
            None => markup::style(
                ICON_WARNING,
                Some(theme::Color::Attention),
                None,
                None,
                None,
            ),
        }
    }
}

#[cfg(test)]
#[expect(clippy::shadow_unrelated)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[test]
    fn test_render() {
        let mut st_config_file = tempfile::NamedTempFile::new().unwrap();
        st_config_file.write_all("<configuration><gui><apikey>dummykeydummykeydummykeydummykey</apikey></gui></configuration>".as_bytes()).unwrap();

        let module = SyncthingModule::new(st_config_file.path()).unwrap();

        let state = Some(SyncthingModuleState {
            folder_count: 1,
            device_connected_count: 2,
            device_syncing_to_count: 3,
            folders_syncing_down_count: 4,
            remote_device_count: 5,
        });
        assert_eq!(
            module.render(&state),
            "%{A1:firefox --new-tab 'http\\://127.0.0.1\\:8384/':}%{F#eee8d5}󱋖%{F-}  1  2/5 4 3%{A}"
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
