use chrono::Datelike;

use crate::config;
use crate::markup;
use crate::polybar_module::{PolybarModuleEnv, RenderablePolybarModule, RuntimeMode};
use crate::theme;

pub struct MarketModule {
    env: PolybarModuleEnv,
    cfg: config::MarketModuleConfig,
}

#[derive(Debug, PartialEq)]
pub struct MarketModuleState {
    api_resp: FmpResponseInnner,
}

type FmpResponse = [FmpResponseInnner; 1];

#[allow(non_snake_case)]
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct FmpResponseInnner {
    price: f64,
    changesPercentage: f64,
    priceAvg50: f64,
    priceAvg200: f64,
}

impl MarketModule {
    pub fn new(cfg: config::MarketModuleConfig) -> MarketModule {
        let env = PolybarModuleEnv::new();
        MarketModule { env, cfg }
    }

    fn wait_working_day() -> bool {
        let mut did_wait = false;
        loop {
            let now = chrono::Local::now();
            let day_num = now.weekday().num_days_from_monday();
            if day_num <= 4 {
                // Weekday
                break;
            }

            // Don't try to be smart by computing absolute time to sleep until,
            // time shifts (NTP, DST...) could easily fuck that up
            std::thread::sleep(std::time::Duration::from_secs(60 * 60 * 8));
            did_wait = true;
        }
        did_wait
    }

    fn try_update(&mut self) -> anyhow::Result<MarketModuleState> {
        // Send request
        let url = reqwest::Url::parse_with_params(
            "https://financialmodelingprep.com/api/v3/quote/%5EFCHI",
            &[("apikey", &self.cfg.api_token)],
        )?;
        log::debug!("{}", url);
        let response = reqwest::blocking::get(url)?.error_for_status()?;

        // Parse response
        let json_data = response.text()?;
        let api_resp: FmpResponse = serde_json::from_str(&json_data)?;

        Ok(MarketModuleState {
            api_resp: api_resp[0].to_owned(),
        })
    }
}

impl RenderablePolybarModule for MarketModule {
    type State = Option<MarketModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if let Some(prev_state) = prev_state {
            std::thread::sleep(match prev_state {
                // Nominal
                Some(_) => std::time::Duration::from_secs(60 * 30),
                // Error occured
                None => std::time::Duration::from_secs(5),
            });
        }
        loop {
            let did_wait_mode = self.env.wait_runtime_mode(RuntimeMode::Unrestricted);
            if prev_state.is_none() {
                break;
            }
            let did_wait_workday = Self::wait_working_day();
            // Yes I know, we could have written this much simplier with a while condition, but we don't want to short circuit
            if !did_wait_mode && !did_wait_workday {
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
            Some(state) => {
                format!(
                    "{} {:.0} {}",
                    markup::style(
                        if state.api_resp.priceAvg50 >= state.api_resp.priceAvg200 {
                            ""
                        } else {
                            ""
                        },
                        Some(theme::Color::MainIcon),
                        None,
                        None,
                        None
                    ),
                    state.api_resp.price,
                    markup::style(
                        &format!("{:+.2}%", state.api_resp.changesPercentage),
                        if state.api_resp.changesPercentage > 1.0 {
                            Some(theme::Color::Good)
                        } else if state.api_resp.changesPercentage < -2.0 {
                            Some(theme::Color::Attention)
                        } else if state.api_resp.changesPercentage < -1.0 {
                            Some(theme::Color::Notice)
                        } else {
                            None
                        },
                        None,
                        None,
                        None
                    ),
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
        let module = MarketModule::new(config::MarketModuleConfig {
            api_token: String::new(),
        });

        let state = Some(MarketModuleState {
            api_resp: FmpResponseInnner {
                price: 5000.6,
                changesPercentage: 0.1,
                priceAvg50: 4501.0,
                priceAvg200: 4500.0,
            },
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 5001 +0.10%");

        let state = Some(MarketModuleState {
            api_resp: FmpResponseInnner {
                price: 5000.6,
                changesPercentage: 0.1,
                priceAvg50: 4500.0,
                priceAvg200: 4501.0,
            },
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 5001 +0.10%");

        let state = Some(MarketModuleState {
            api_resp: FmpResponseInnner {
                price: 5000.6,
                changesPercentage: 1.01,
                priceAvg50: 4501.0,
                priceAvg200: 4500.0,
            },
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 5001 %{F#859900}+1.01%%{F-}"
        );

        let state = Some(MarketModuleState {
            api_resp: FmpResponseInnner {
                price: 5000.6,
                changesPercentage: -2.01,
                priceAvg50: 4501.0,
                priceAvg200: 4500.0,
            },
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 5001 %{F#cb4b16}-2.01%%{F-}"
        );

        let state = Some(MarketModuleState {
            api_resp: FmpResponseInnner {
                price: 5000.6,
                changesPercentage: -1.01,
                priceAvg50: 4501.0,
                priceAvg200: 4500.0,
            },
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 5001 %{F#b58900}-1.01%%{F-}"
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
