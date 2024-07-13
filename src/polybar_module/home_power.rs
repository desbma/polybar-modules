use std::{
    cmp::Ordering,
    net::{TcpStream, ToSocketAddrs},
    thread::sleep,
    time::Duration,
};

use backoff::backoff::Backoff;
use itertools::Itertools;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::Digest as _;
use tungstenite::WebSocket;

use crate::{
    config::{HomePowerModuleConfig, ShellyDeviceConfig},
    markup,
    polybar_module::{NetworkMode, PolybarModuleEnv, RenderablePolybarModule, TCP_REMOTE_TIMEOUT},
    theme,
};

pub(crate) struct HomePowerModule {
    se_client: reqwest::blocking::Client,
    se_req_power_flow: reqwest::blocking::Request,
    se_wait_delay: Option<Duration>,
    shelly_devices: Vec<(ShellyDeviceConfig, Option<ShellyPlus>)>,
    env: PolybarModuleEnv,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct HomePowerModuleState {
    solar_power: u32,
    home_consumption_power: u32,
    grid_power: u32,
    devices: Vec<HomeDevice>,
}

#[derive(Debug, Eq, PartialEq)]
struct HomeDevice {
    name: String,
    status: Option<HomeDeviceStatus>,
}

#[derive(Debug, Eq, PartialEq)]
struct HomeDeviceStatus {
    enabled: bool,
    power: u32,
}

//
// SolarEdge API
//

#[derive(Debug, Deserialize)]
#[serde(rename_all(deserialize = "camelCase"))]
struct SeSiteCurrentPowerFlow {
    update_refresh_rate: u64,
    #[serde(alias = "GRID", alias = "grid")]
    grid: SePowerState,
    #[serde(alias = "LOAD", alias = "load")]
    load: SePowerState,
    #[serde(alias = "PV", alias = "pv")]
    pv: SePowerState,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all(deserialize = "camelCase"))]
struct SePowerState {
    current_power: f64,
}

//
// Shelly API
//

const SHELLY_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const SHELLY_RECV_TIMEOUT: Duration = Duration::from_secs(3);
const SHELLY_SEND_TIMEOUT: Duration = Duration::from_secs(3);

struct ShellyPlus {
    ws: WebSocket<TcpStream>,
    auth: Option<ShellyRpcAuthChallengeResponse>,
    next_msg_id: u64,
    password: String,
}

#[derive(Debug, Serialize)]
struct ShellyRpcRequest<P> {
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth: Option<ShellyRpcAuthChallengeResponse>,
    params: P,
}

#[derive(Debug, Clone, Serialize)]
struct ShellyRpcAuthChallengeResponse {
    realm: String,
    username: String,
    nonce: u64,
    cnonce: u64,
    response: String,
    algorithm: String,
}

#[derive(Debug, Serialize)]
struct ShellyRpcParamsSwitchGetStatus {
    id: u64,
}

#[derive(Debug, Deserialize)]
struct ShellyRpcResponse<T> {
    #[serde(flatten)]
    result: ShellyRpcResponseResult<T>,
}

#[derive(Debug, Deserialize)]
enum ShellyRpcResponseResult<T> {
    #[serde(rename = "result")]
    Success(T),
    #[serde(rename = "error")]
    Error { code: u64, message: String },
}

#[derive(Debug, Deserialize)]
struct ShellyRpcAuthParams {
    nonce: u64,
    nc: u64,
    realm: String,
    algorithm: String,
}

#[derive(Debug, serde::Deserialize)]
struct ShellyRpcResultSwitchStatus {
    output: bool,
    apower: Option<f64>,
}

fn sha256_hex(s: &str) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(s.as_bytes());
    let hash = hasher.finalize();
    hex::encode(hash)
}

impl ShellyPlus {
    fn connect(host: &str, password: &str) -> anyhow::Result<Self> {
        let host_port = format!("{host}:80");
        let addr = host_port.to_socket_addrs()?.next().unwrap();
        let stream = TcpStream::connect_timeout(&addr, SHELLY_CONNECT_TIMEOUT)?;
        stream.set_read_timeout(Some(SHELLY_RECV_TIMEOUT))?;
        stream.set_write_timeout(Some(SHELLY_SEND_TIMEOUT))?;
        let url = format!("ws://{host}/rpc");
        let ws = tungstenite::client(url, stream)?.0;

        Ok(Self {
            ws,
            auth: None,
            next_msg_id: 0,
            password: password.to_owned(),
        })
    }

    // Send a request, authenticate if needed, and parse response
    #[allow(clippy::shadow_unrelated)]
    fn request<P, R>(&mut self, call: &str, params: P) -> anyhow::Result<R>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        //
        // See https://shelly-api-docs.shelly.cloud/gen2/General/Authentication#authentication-process
        //

        // Send message
        let req: ShellyRpcRequest<P> = ShellyRpcRequest {
            id: self.next_msg_id,
            method: call.to_owned(),
            auth: self.auth.clone(),
            params,
        };
        self.next_msg_id += 1;
        self.ws
            .send(tungstenite::Message::Text(serde_json::to_string(&req)?))?;

        // Parse response
        let resp = self.recv_msg()?.into_text()?;
        let api_resp: ShellyRpcResponse<R> = serde_json::from_str(&resp)?;
        match api_resp.result {
            ShellyRpcResponseResult::Success(result) => Ok(result),
            ShellyRpcResponseResult::Error { code: 401, message } => {
                // We need to (re)authenticate
                log::debug!("401 error message received, (re)authenticating");

                // Parse auth params
                let api_auth_params: ShellyRpcAuthParams = serde_json::from_str(&message)?;

                // Build challenge response
                let cnonce = rand::random();
                let ha1 = sha256_hex(&format!(
                    "admin:{}:{}",
                    api_auth_params.realm, self.password
                ));
                let ha2 = sha256_hex("dummy_method:dummy_uri");
                let response = sha256_hex(&format!(
                    "{}:{}:{}:{}:auth:{}",
                    ha1, api_auth_params.nonce, api_auth_params.nc, cnonce, ha2
                ));
                let auth_resp = ShellyRpcAuthChallengeResponse {
                    realm: api_auth_params.realm,
                    username: "admin".to_owned(),
                    nonce: api_auth_params.nonce,
                    cnonce,
                    response,
                    algorithm: api_auth_params.algorithm,
                };

                // Send original request with challenge response
                let req: ShellyRpcRequest<P> = ShellyRpcRequest {
                    id: self.next_msg_id,
                    auth: Some(auth_resp.clone()),
                    ..req
                };
                self.next_msg_id += 1;
                let req_str = serde_json::to_string(&req)?;
                self.ws.send(tungstenite::Message::Text(req_str))?;

                // Parse response
                let resp = self.recv_msg()?.into_text()?;
                let api_resp: ShellyRpcResponse<R> = serde_json::from_str(&resp)?;
                match api_resp.result {
                    ShellyRpcResponseResult::Success(result) => {
                        log::debug!("Successfully authenticated");
                        self.auth = Some(auth_resp);
                        Ok(result)
                    }
                    ShellyRpcResponseResult::Error { code, message } => Err(anyhow::anyhow!(
                        "Request failed with code {code}: {message:?}"
                    )),
                }
            }
            ShellyRpcResponseResult::Error { code, message } => Err(anyhow::anyhow!(
                "Request failed with code {code}: {message:?}"
            )),
        }
    }

    /// Receive message and ignore pings
    fn recv_msg(&mut self) -> anyhow::Result<tungstenite::Message> {
        loop {
            let msg = self.ws.read()?;
            if !msg.is_empty() {
                break Ok(msg);
            }
        }
    }

    fn get_switch_status(&mut self) -> anyhow::Result<ShellyRpcResultSwitchStatus> {
        self.request("Switch.GetStatus", ShellyRpcParamsSwitchGetStatus { id: 0 })
    }
}

impl HomePowerModule {
    pub(crate) fn new(cfg: &HomePowerModuleConfig) -> anyhow::Result<Self> {
        let se_client = reqwest::blocking::Client::builder()
            .timeout(TCP_REMOTE_TIMEOUT)
            .build()?;
        // Web API used by the monitoring web site
        // Does not seem to be rate limited, unlike the official API
        let se_url = format!(
            "https://monitoring.solaredge.com/services/powerflow/site/{}/latest",
            cfg.se.site_id
        );
        let se_auth_cookie = format!(
            "SPRING_SECURITY_REMEMBER_ME_COOKIE={};",
            cfg.se.auth_cookie_val
        );
        let se_req_power_flow = se_client
            .get(se_url)
            .header("Cookie", &se_auth_cookie)
            .build()?;

        let shelly_devices = cfg
            .shelly_devices
            .iter()
            .map(|d| (d.to_owned(), None))
            .collect();

        let env = PolybarModuleEnv::new();
        Ok(Self {
            se_client,
            se_req_power_flow,
            se_wait_delay: None,
            shelly_devices,
            env,
        })
    }

    fn try_update(&mut self) -> anyhow::Result<HomePowerModuleState> {
        let text = self
            .se_client
            .execute(self.se_req_power_flow.try_clone().unwrap())?
            .error_for_status()?
            .text()?;
        log::debug!("{:?}", text);
        let site_data: SeSiteCurrentPowerFlow = serde_json::from_str(&text)?;
        log::debug!("{:?}", site_data);

        // The rate limit is not as aggressive with this API, use the upstream delay typically of 3s
        self.se_wait_delay = Some(Duration::from_secs(site_data.update_refresh_rate));

        let devices = self
            .shelly_devices
            .iter_mut()
            .map(|(cfg, dev)| {
                if dev.is_none() {
                    *dev = ShellyPlus::connect(&cfg.host, &cfg.password)
                        .inspect_err(|e| log::warn!("Connecting to {:?} failed: {}", cfg.host, e))
                        .ok();
                }
                if let Some(status) = dev.as_mut().and_then(|d| {
                    d.get_switch_status()
                        .inspect_err(|e| {
                            log::warn!("Getting status of {:?} failed: {}", cfg.host, e);
                        })
                        .ok()
                }) {
                    HomeDevice {
                        name: cfg.name.clone(),
                        status: Some(HomeDeviceStatus {
                            enabled: status.output,
                            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                            power: status.apower.map_or(0, |v| v as u32),
                        }),
                    }
                } else {
                    *dev = None;
                    HomeDevice {
                        name: cfg.name.clone(),
                        status: None,
                    }
                }
            })
            .collect();

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(HomePowerModuleState {
            solar_power: (site_data.pv.current_power * 1000.0) as u32,
            home_consumption_power: (site_data.load.current_power * 1000.0) as u32,
            grid_power: (site_data.grid.current_power * 1000.0) as u32,
            devices,
        })
    }
}

impl RenderablePolybarModule for HomePowerModule {
    type State = Option<HomePowerModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if let Some(prev_state) = prev_state {
            let sleep_duration = match prev_state {
                // Nominal
                Some(_) => {
                    self.env.network_error_backoff.reset();
                    if let Some(wait_delay) = self.se_wait_delay {
                        wait_delay
                    } else {
                        Duration::from_secs(60)
                    }
                }
                // Error occured
                None => self.env.network_error_backoff.next_backoff().unwrap(),
            };
            sleep(sleep_duration);
        }
        self.env.wait_network_mode(&NetworkMode::Unrestricted);
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
                format!(
                    "{} {:.1}{}󱤃{:.1}{}󰴾{:.1}kW{}",
                    markup::style("", Some(theme::Color::MainIcon), None, None, None),
                    f64::from(state.solar_power) / 1000.0,
                    if state.solar_power > 0 { '' } else { ' ' },
                    f64::from(state.home_consumption_power) / 1000.0,
                    match state.solar_power.cmp(&state.home_consumption_power) {
                        Ordering::Greater => '',
                        Ordering::Less => '',
                        Ordering::Equal => ' ',
                    },
                    f64::from(state.grid_power) / 1000.0,
                    if state.devices.is_empty() {
                        String::new()
                    } else {
                        format!(
                            " {}",
                            state
                                .devices
                                .iter()
                                .map(|d| {
                                    markup::style(
                                        &d.name,
                                        d.status.is_none().then_some(theme::Color::Attention),
                                        if d.status
                                            .as_ref()
                                            .is_some_and(|s| s.enabled && s.power > 0)
                                        {
                                            Some(theme::Color::Notice)
                                        } else if d.status.as_ref().is_some_and(|s| s.enabled) {
                                            Some(theme::Color::Foreground)
                                        } else {
                                            None
                                        },
                                        None,
                                        None,
                                    )
                                })
                                .join(" ")
                        )
                    }
                )
            }
            None => markup::style("", Some(theme::Color::Attention), None, None, None),
        }
    }
}

#[cfg(test)]
#[allow(clippy::shadow_unrelated)]
mod tests {
    use super::*;
    use crate::config::SolarEdgeConfig;

    #[test]
    fn test_render() {
        let module = HomePowerModule::new(&HomePowerModuleConfig {
            se: SolarEdgeConfig {
                site_id: 0,
                auth_cookie_val: String::new(),
            },
            shelly_devices: vec![],
        })
        .unwrap();

        let state = Some(HomePowerModuleState {
            solar_power: 2000,
            home_consumption_power: 600,
            grid_power: 1400,
            devices: vec![],
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}\u{ea06}%{F-} \u{ee81}2.0\u{e912}\u{f1903}0.6\u{e912}\u{f0d3e}1.4kW"
        );

        let state = Some(HomePowerModuleState {
            solar_power: 0,
            home_consumption_power: 600,
            grid_power: 1400,
            devices: vec![
                HomeDevice {
                    name: "D1".to_owned(),
                    status: Some(HomeDeviceStatus {
                        enabled: false,
                        power: 0,
                    }),
                },
                HomeDevice {
                    name: "D2".to_owned(),
                    status: Some(HomeDeviceStatus {
                        enabled: true,
                        power: 0,
                    }),
                },
                HomeDevice {
                    name: "D3".to_owned(),
                    status: Some(HomeDeviceStatus {
                        enabled: true,
                        power: 1500,
                    }),
                },
                HomeDevice {
                    name: "D4".to_owned(),
                    status: None,
                },
            ],
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}\u{ea06}%{F-} \u{ee81}0.0 \u{f1903}0.6\u{e910}\u{f0d3e}1.4kW D1 %{u#93a1a1}%{+u}D2%{-u} %{u#b58900}%{+u}D3%{-u} %{F#cb4b16}D4%{F-}"
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
