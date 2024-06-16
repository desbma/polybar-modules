use std::{
    cmp::{max, Ordering},
    thread::sleep,
    time::Duration,
};

use backoff::backoff::Backoff;
use itertools::Itertools;
use serde::Deserialize;

use crate::{
    config::HomePowerModuleConfig,
    markup,
    polybar_module::{NetworkMode, PolybarModuleEnv, RenderablePolybarModule, TCP_REMOTE_TIMEOUT},
    theme,
};

pub struct HomePowerModule {
    client: reqwest::blocking::Client,
    req_power_flow: reqwest::blocking::Request,
    req_devices: reqwest::blocking::Request,
    wait_delay: Option<Duration>,
    env: PolybarModuleEnv,
}

#[derive(Debug, Eq, PartialEq)]
pub struct HomePowerModuleState {
    pub solar_power: u32,
    pub home_consumption_power: u32,
    pub grid_power: u32,
    pub devices: Vec<HomeDevice>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct HomeDevice {
    name: String,
    enabled: bool,
    power: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all(deserialize = "camelCase"))]
struct SiteDevices {
    update_refresh_rate: u64,
    devices: Vec<SiteDevice>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all(deserialize = "camelCase"))]
struct SiteDevice {
    name: String,
    status: DeviceStatus,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all(deserialize = "camelCase"))]
struct DeviceStatus {
    level: Option<u64>,
    #[allow(dead_code)]
    active_power_meter: Option<f64>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all(deserialize = "camelCase"))]
struct SiteCurrentPowerFlow {
    update_refresh_rate: u64,
    connections: Vec<PowerConnection>,
    #[serde(alias = "GRID", alias = "grid")]
    grid: PowerState,
    #[serde(alias = "LOAD", alias = "load")]
    load: PowerState,
    #[serde(alias = "PV", alias = "pv")]
    pv: PowerState,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all(deserialize = "camelCase"))]
struct PowerConnection {
    from: String,
    to: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all(deserialize = "camelCase"))]
struct PowerState {
    status: PowerStatus,
    current_power: f64,
}

#[derive(Debug, Deserialize)]
enum PowerStatus {
    Active,
    Idle,
    Disabled,
}

impl HomePowerModule {
    pub fn new(cfg: HomePowerModuleConfig) -> anyhow::Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(TCP_REMOTE_TIMEOUT)
            .build()?;
        // Web API used by the monitoring web site
        // Does not seem to be rate limited, unlike the official API
        let url = format!(
            "https://monitoring.solaredge.com/services/powerflow/site/{}/latest",
            cfg.site_id
        );
        let auth_cookie = format!(
            "SPRING_SECURITY_REMEMBER_ME_COOKIE={};",
            cfg.auth_cookie_val
        );
        let req_power_flow = client.get(url).header("Cookie", &auth_cookie).build()?;
        let url = format!(
            "https://ha.monitoring.solaredge.com/api/homeautomation/v1.0/sites/{}/devices",
            cfg.site_id
        );
        let req_devices = client.get(url).header("Cookie", &auth_cookie).build()?;
        let env = PolybarModuleEnv::new();
        Ok(Self {
            client,
            req_power_flow,
            req_devices,
            wait_delay: None,
            env,
        })
    }

    fn try_update(&mut self) -> anyhow::Result<HomePowerModuleState> {
        let text = self
            .client
            .execute(self.req_power_flow.try_clone().unwrap())?
            .error_for_status()?
            .text()?;
        log::debug!("{:?}", text);
        let site_data: SiteCurrentPowerFlow = serde_json::from_str(&text)?;
        log::debug!("{:?}", site_data);

        let text = self
            .client
            .execute(self.req_devices.try_clone().unwrap())?
            .error_for_status()?
            .text()?;
        log::debug!("{:?}", text);
        let devices_data: SiteDevices = serde_json::from_str(&text)?;
        log::debug!("{:?}", devices_data);

        // The rate limit is not as aggressive with this API, use the upstream delay typically of 3s
        self.wait_delay = Some(Duration::from_secs(max(
            site_data.update_refresh_rate,
            devices_data.update_refresh_rate,
        )));

        Ok(HomePowerModuleState {
            solar_power: (site_data.pv.current_power * 1000.0) as u32,
            home_consumption_power: (site_data.load.current_power * 1000.0) as u32,
            grid_power: (site_data.grid.current_power * 1000.0) as u32,
            devices: devices_data
                .devices
                .into_iter()
                .filter(|d| d.status.level.is_some())
                .map(|d| HomeDevice {
                    name: d.name.clone(),
                    enabled: d.status.level.unwrap() > 0,
                    power: 0, //d.status.active_power_meter.unwrap_or(0.0) as u32,
                })
                .collect(),
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
                    if let Some(wait_delay) = self.wait_delay {
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
        self.env.wait_network_mode(NetworkMode::Unrestricted);
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
                    state.solar_power as f64 / 1000.0,
                    if state.solar_power > 0 { '' } else { ' ' },
                    state.home_consumption_power as f64 / 1000.0,
                    match state.solar_power.cmp(&state.home_consumption_power) {
                        Ordering::Greater => '',
                        Ordering::Less => '',
                        Ordering::Equal => ' ',
                    },
                    state.grid_power as f64 / 1000.0,
                    if state.devices.is_empty() {
                        "".to_string()
                    } else {
                        format!(
                            " {}",
                            state
                                .devices
                                .iter()
                                .map(|d| {
                                    markup::style(
                                        &d.name,
                                        None,
                                        if d.enabled && (d.power > 0) {
                                            Some(theme::Color::Notice)
                                        } else if d.enabled {
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
mod tests {
    use super::*;

    #[test]
    fn test_render() {
        let module = HomePowerModule::new(HomePowerModuleConfig {
            site_id: 0,
            auth_cookie_val: "".to_string(),
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
                    name: "D1".to_string(),
                    enabled: false,
                    power: 0,
                },
                HomeDevice {
                    name: "D2".to_string(),
                    enabled: true,
                    power: 0,
                },
                HomeDevice {
                    name: "D3".to_string(),
                    enabled: true,
                    power: 1500,
                },
            ],
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}\u{ea06}%{F-} \u{ee81}0.0 \u{f1903}0.6\u{e910}\u{f0d3e}1.4kW D1 %{u#93a1a1}%{+u}D2%{-u} %{u#b58900}%{+u}D3%{-u}"
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
