use std::thread::sleep;
use std::time::Duration;

use backoff::backoff::Backoff;
use serde::Deserialize;

use crate::config::HomePowerModuleConfig;
use crate::markup;
use crate::polybar_module::{
    NetworkMode, PolybarModuleEnv, RenderablePolybarModule, TCP_REMOTE_TIMEOUT,
};
use crate::theme;

pub struct HomePowerModule {
    client: reqwest::blocking::Client,
    req: reqwest::blocking::Request,
    env: PolybarModuleEnv,
}

#[derive(Debug, Eq, PartialEq)]
pub struct HomePowerModuleState {
    pub solar_power: u32,
    pub home_consumption_power: u32,
    pub grid_power: u32,
    refresh_delay: Duration,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all(deserialize = "camelCase"))]
struct CurrentPowerFlowResponse {
    site_current_power_flow: SiteCurrentPowerFlow,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all(deserialize = "camelCase"))]
struct SiteCurrentPowerFlow {
    update_refresh_rate: u64,
    connections: Vec<PowerConnection>,
    #[serde(rename(deserialize = "GRID"))]
    grid: PowerState,
    #[serde(rename(deserialize = "LOAD"))]
    load: PowerState,
    #[serde(rename(deserialize = "PV"))]
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
        let url = reqwest::Url::parse_with_params(
            &format!(
                "https://monitoringapi.solaredge.com/site/{}/currentPowerFlow.json",
                cfg.site_id
            ),
            &[("api_key", &cfg.api_key)],
        )?;
        let req = client.get(url).build()?;
        let env = PolybarModuleEnv::new();
        Ok(Self { client, req, env })
    }

    fn try_update(&mut self) -> anyhow::Result<HomePowerModuleState> {
        let text = self
            .client
            .execute(self.req.try_clone().unwrap())?
            .error_for_status()?
            .text()?;
        log::debug!("{:?}", text);
        let resp: CurrentPowerFlowResponse = serde_json::from_str(&text)?;
        log::debug!("{:?}", resp);

        Ok(HomePowerModuleState {
            solar_power: (resp.site_current_power_flow.pv.current_power * 1000.0) as u32,
            home_consumption_power: (resp.site_current_power_flow.load.current_power * 1000.0)
                as u32,
            grid_power: (resp.site_current_power_flow.grid.current_power * 1000.0) as u32,
            refresh_delay: Duration::from_secs(resp.site_current_power_flow.update_refresh_rate),
        })
    }
}

impl RenderablePolybarModule for HomePowerModule {
    type State = Option<HomePowerModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if let Some(prev_state) = prev_state {
            let sleep_duration = match prev_state {
                // Nominal
                Some(prev_state) => {
                    self.env.network_error_backoff.reset();
                    prev_state.refresh_delay
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
                    "{} {:.1}{:.1}{}{:.1}kW",
                    markup::style("", Some(theme::Color::MainIcon), None, None, None),
                    state.solar_power as f64 / 1000.0,
                    state.home_consumption_power as f64 / 1000.0,
                    if state.solar_power > state.home_consumption_power {
                        ""
                    } else {
                        ""
                    },
                    state.grid_power as f64 / 1000.0,
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
            api_key: "".to_string(),
        })
        .unwrap();

        let state = Some(HomePowerModuleState {
            solar_power: 2000,
            home_consumption_power: 600,
            grid_power: 1400,
            refresh_delay: Duration::from_secs(3),
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}\u{ea06}%{F-} \u{e9d7}2.0\u{e912}\u{e979}0.6\u{e912}\u{e954}1.4kW"
        );

        let state = Some(HomePowerModuleState {
            solar_power: 0,
            home_consumption_power: 600,
            grid_power: 1400,
            refresh_delay: Duration::from_secs(3),
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}\u{ea06}%{F-} \u{e9d7}0.0\u{e912}\u{e979}0.6\u{e910}\u{e954}1.4kW"
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
