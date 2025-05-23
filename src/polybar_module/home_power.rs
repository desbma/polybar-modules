use std::{
    cmp::Ordering,
    net::{SocketAddr, TcpStream, ToSocketAddrs as _},
    thread::sleep,
    time::Duration,
};

use anyhow::Context as _;
use backoff::backoff::Backoff as _;
use itertools::Itertools as _;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::Digest as _;
use tokio_modbus::prelude::SyncReader as _;
use tungstenite::WebSocket;

use crate::{
    config::{HomePowerModuleConfig, ShellyDeviceConfig},
    markup,
    polybar_module::{NetworkMode, PolybarModuleEnv, RenderablePolybarModule},
    theme,
};

pub(crate) struct HomePowerModule {
    modbus_addr: SocketAddr,
    modbus_ctx: Option<tokio_modbus::client::sync::Context>,
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
    #[expect(clippy::shadow_unrelated)]
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
        self.ws.send(tungstenite::Message::Text(
            serde_json::to_string(&req)?.into(),
        ))?;

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
                self.ws.send(tungstenite::Message::Text(req_str.into()))?;

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
        let addr = format!("{}:{}", cfg.inverter_modbus.host, cfg.inverter_modbus.port)
            .to_socket_addrs()?
            .at_most_one()
            .ok()
            .flatten()
            .ok_or_else(|| anyhow::anyhow!("Inverser IP resolution did not yield 1 IP"))?;

        let shelly_devices = cfg
            .shelly_devices
            .iter()
            .map(|d| (d.to_owned(), None))
            .collect();

        let env = PolybarModuleEnv::new();
        Ok(Self {
            modbus_addr: addr,
            modbus_ctx: None,
            shelly_devices,
            env,
        })
    }

    #[expect(clippy::cast_possible_wrap)]
    fn try_update(&mut self) -> anyhow::Result<HomePowerModuleState> {
        let modbus_ctx = if let Some(modbus_ctx) = self.modbus_ctx.as_mut() {
            modbus_ctx
        } else {
            let modbus_ctx =
                tokio_modbus::client::sync::tcp::connect_slave(self.modbus_addr, 1.into())
                    .context("Failed to connect to inverter")?;
            self.modbus_ctx = Some(modbus_ctx);
            self.modbus_ctx.as_mut().unwrap()
        };

        // https://github.com/nmakel/solaredge_modbus/blob/fd3ce7ae32a259ee371c672dac3bcd75bfe51258/src/solaredge_modbus/__init__.py#L486
        let power_ac = modbus_ctx
            .read_holding_registers(0x9c93, 1)??
            .into_iter()
            .at_most_one()
            .ok()
            .flatten()
            .unwrap() as i16;
        let power_ac_scale = modbus_ctx
            .read_holding_registers(0x9c94, 1)??
            .into_iter()
            .at_most_one()
            .ok()
            .flatten()
            .unwrap() as i16;
        let solar_power = f64::from(power_ac) * 10_f64.powf(f64::from(power_ac_scale));

        // https://github.com/nmakel/solaredge_modbus/blob/fd3ce7ae32a259ee371c672dac3bcd75bfe51258/src/solaredge_modbus/__init__.py#L603
        let l1_power = modbus_ctx
            .read_holding_registers(0x9d0f, 1)??
            .into_iter()
            .at_most_one()
            .ok()
            .flatten()
            .unwrap() as i16;
        let power_scale = modbus_ctx
            .read_holding_registers(0x9d12, 1)??
            .into_iter()
            .at_most_one()
            .ok()
            .flatten()
            .unwrap() as i16;
        let grid_export = f64::from(l1_power) * 10_f64.powf(f64::from(power_scale));

        let home_consumption_power = solar_power - grid_export;

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
                            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
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

        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(HomePowerModuleState {
            solar_power: solar_power as u32,
            home_consumption_power: home_consumption_power as u32,
            grid_power: grid_export.abs() as u32,
            devices,
        })
    }
}

impl RenderablePolybarModule for HomePowerModule {
    type State = Option<HomePowerModuleState>;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        if let Some(prev_state) = prev_state {
            let sleep_duration = if prev_state.is_some() {
                self.env.network_error_backoff.reset();
                Duration::from_secs(1)
            } else {
                self.modbus_ctx = None; // Force reconnect
                self.env.network_error_backoff.next_backoff().unwrap()
            };
            sleep(sleep_duration);
        }
        self.env.wait_network_mode(&NetworkMode::Unrestricted);
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
                                        d.status.is_none().then_some(theme::Color::Unfocused),
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
#[expect(clippy::shadow_unrelated)]
mod tests {
    use super::*;
    use crate::config::InverterModbusConfig;

    #[test]
    fn test_render() {
        let module = HomePowerModule::new(&HomePowerModuleConfig {
            shelly_devices: vec![],
            inverter_modbus: InverterModbusConfig {
                host: "127.0.0.1".to_owned(),
                port: 0,
            },
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
            "%{F#eee8d5}\u{ea06}%{F-} \u{ee81}0.0 \u{f1903}0.6\u{e910}\u{f0d3e}1.4kW D1 %{u#93a1a1}%{+u}D2%{-u} %{u#b58900}%{+u}D3%{-u} %{F#657b83}D4%{F-}"
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
