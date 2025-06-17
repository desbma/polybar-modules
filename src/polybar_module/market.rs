use std::{thread::sleep, time::Duration};

use anyhow::Context as _;
use backon::BackoffBuilder as _;
use chrono::Datelike as _;

use crate::{
    markup,
    polybar_module::{NetworkMode, PolybarModuleEnv, RenderablePolybarModule, TCP_REMOTE_TIMEOUT},
    theme::{self, ICON_WARNING},
};

pub(crate) struct MarketModule {
    client: ureq::Agent,
    selector_val: scraper::Selector,
    selector_delta: scraper::Selector,
    selector_ma50: scraper::Selector,
    selector_ma100: scraper::Selector,
    env: PolybarModuleEnv,
}

#[derive(Debug, PartialEq)]
pub(crate) struct MarketModuleState {
    val: f64,
    delta_prct: f64,
    ma50: f64,
    ma100: f64,
}

impl MarketModule {
    pub(crate) fn new() -> Self {
        let client = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .tls_config(
                    ureq::tls::TlsConfig::builder()
                        .provider(ureq::tls::TlsProvider::NativeTls)
                        .build(),
                )
                .timeout_global(Some(TCP_REMOTE_TIMEOUT))
                .build(),
        );

        // TODO improve selectors?
        let selector_val = scraper::Selector::parse(
            ".l-quotepage__header .c-faceplate__price > span:nth-child(1)",
        )
        .unwrap();
        let selector_delta = scraper::Selector::parse(
            ".l-quotepage__header .c-faceplate__fluctuation .c-instrument--variation",
        )
        .unwrap();
        let selector_ma50 =
            scraper::Selector::parse("tr.c-table__row:nth-child(11) > td:nth-child(4)").unwrap();
        let selector_ma100 =
            scraper::Selector::parse("tr.c-table__row:nth-child(12) > td:nth-child(4)").unwrap();
        let env = PolybarModuleEnv::new();

        Self {
            client,
            selector_val,
            selector_delta,
            selector_ma50,
            selector_ma100,
            env,
        }
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
            sleep(Duration::from_secs(60 * 60 * 8));
            did_wait = true;
        }
        did_wait
    }

    fn extract_float(page: &scraper::Html, sel: &scraper::Selector) -> anyhow::Result<f64> {
        let mut val_str = page
            .select(sel)
            .next()
            .ok_or_else(|| anyhow::anyhow!("Failed to find value in HTML"))?
            .inner_html()
            .replace(',', ".")
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();
        if let Some(new_val_str) = val_str.strip_suffix('%') {
            val_str = new_val_str.to_owned();
        }
        let val = val_str
            .parse()
            .with_context(|| format!("Failed to parse {val_str:?}"))?;
        Ok(val)
    }

    fn try_update(&mut self) -> anyhow::Result<MarketModuleState> {
        // Send request
        let url = "https://www.boursorama.com/bourse/indices/cours/1rPCAC/";
        let response = self.client.get(url).call()?;
        anyhow::ensure!(
            response.status().is_success(),
            "HTTP response {}",
            response.status(),
        );

        // Parse response
        let page = scraper::Html::parse_document(&response.into_body().read_to_string()?);
        let val =
            Self::extract_float(&page, &self.selector_val).context("Failed to extract value")?;
        let delta_prct =
            Self::extract_float(&page, &self.selector_delta).context("Failed to extract delta")?;
        let ma50 =
            Self::extract_float(&page, &self.selector_ma50).context("Failed to extract MA50")?;
        let ma100 =
            Self::extract_float(&page, &self.selector_ma100).context("Failed to extract MA100")?;

        Ok(MarketModuleState {
            val,
            delta_prct,
            ma50,
            ma100,
        })
    }
}

const ICON_MARKET_UP: &str = "";
const ICON_MARKET_DOWN: &str = "";

impl RenderablePolybarModule for MarketModule {
    type State = Option<MarketModuleState>;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        if let Some(prev_state) = prev_state {
            let sleep_duration = match prev_state {
                // Nominal
                Some(_) => {
                    self.env.network_error_backoff = self.env.network_error_backoff_builder.build();
                    Duration::from_secs(60 * 30)
                }
                // Error occured
                None => self.env.network_error_backoff.next().unwrap(),
            };
            sleep(sleep_duration);
        }
        loop {
            let did_wait_mode = self.env.wait_network_mode(&NetworkMode::Unrestricted);
            match prev_state {
                Some(None) | None => break,
                _ => {}
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
                log::error!("{e}");
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
                        if state.ma50 >= state.ma100 {
                            ICON_MARKET_UP
                        } else {
                            ICON_MARKET_DOWN
                        },
                        Some(theme::Color::MainIcon),
                        None,
                        None,
                        None
                    ),
                    state.val,
                    markup::style(
                        &format!("{:+.2}%", state.delta_prct),
                        if state.delta_prct > 1.0 {
                            Some(theme::Color::Good)
                        } else if state.delta_prct < -2.0 {
                            Some(theme::Color::Attention)
                        } else if state.delta_prct < -1.0 {
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
    use super::*;

    #[test]
    fn test_render() {
        let module = MarketModule::new();

        let state = Some(MarketModuleState {
            val: 5000.6,
            delta_prct: 0.1,
            ma50: 4501.0,
            ma100: 4500.0,
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 5001 +0.10%");

        let state = Some(MarketModuleState {
            val: 5000.6,
            delta_prct: 0.1,
            ma50: 4500.0,
            ma100: 4501.0,
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 5001 +0.10%");

        let state = Some(MarketModuleState {
            val: 5000.6,
            delta_prct: 1.01,
            ma50: 4501.0,
            ma100: 4500.0,
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 5001 %{F#859900}+1.01%%{F-}"
        );

        let state = Some(MarketModuleState {
            val: 5000.6,
            delta_prct: -2.01,
            ma50: 4501.0,
            ma100: 4500.0,
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 5001 %{F#cb4b16}-2.01%%{F-}"
        );

        let state = Some(MarketModuleState {
            val: 5000.6,
            delta_prct: -1.01,
            ma50: 4501.0,
            ma100: 4500.0,
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 5001 %{F#b58900}-1.01%%{F-}"
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
