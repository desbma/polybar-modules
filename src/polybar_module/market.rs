use std::thread::sleep;
use std::time::Duration;

use anyhow::Context;
use chrono::Datelike;

use crate::markup;
use crate::polybar_module::{PolybarModuleEnv, RenderablePolybarModule, RuntimeMode};
use crate::theme;

pub struct MarketModule {
    url: reqwest::Url,
    selector_val: scraper::Selector,
    selector_delta: scraper::Selector,
    selector_ma50: scraper::Selector,
    selector_ma100: scraper::Selector,
    env: PolybarModuleEnv,
}

#[derive(Debug, PartialEq)]
pub struct MarketModuleState {
    val: f64,
    delta_prct: f64,
    ma50: f64,
    ma100: f64,
}

impl MarketModule {
    pub fn new() -> MarketModule {
        let url =
            reqwest::Url::parse("https://www.boursorama.com/bourse/indices/cours/1rPCAC/").unwrap();
        // TODO improve selectors?
        let selector_val =
            scraper::Selector::parse(".c-faceplate__price > span:nth-child(1)").unwrap();
        let selector_delta =
            scraper::Selector::parse("span.u-color-stream-down > span:nth-child(1)").unwrap();
        let selector_ma50 =
            scraper::Selector::parse("tr.c-table__row:nth-child(11) > td:nth-child(4)").unwrap();
        let selector_ma100 =
            scraper::Selector::parse("tr.c-table__row:nth-child(12) > td:nth-child(4)").unwrap();
        let env = PolybarModuleEnv::new();
        MarketModule {
            url,
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

    fn try_update(&mut self) -> anyhow::Result<MarketModuleState> {
        // Send request
        log::debug!("{}", self.url);
        let response = reqwest::blocking::get(self.url.clone())?.error_for_status()?;

        // Parse response
        let page = scraper::Html::parse_document(&response.text()?);
        let val_str = page
            .select(&self.selector_val)
            .next()
            .ok_or_else(|| anyhow::anyhow!("Failed to find value"))?
            .inner_html()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();
        let val = val_str
            .parse()
            .context(format!("Failed to parse {:?}", val_str))?;
        let delta_prct_str = page
            .select(&self.selector_delta)
            .next()
            .ok_or_else(|| anyhow::anyhow!("Failed to find delta"))?
            .inner_html()
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '%')
            .collect::<String>();
        let delta_prct = delta_prct_str
            .parse()
            .context(format!("Failed to parse {:?}", delta_prct_str))?;
        let ma50_str = page
            .select(&self.selector_ma50)
            .next()
            .ok_or_else(|| anyhow::anyhow!("Failed to find MA50"))?
            .inner_html()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();
        let ma50 = ma50_str
            .parse()
            .context(format!("Failed to parse {:?}", ma50_str))?;
        let ma100_str = page
            .select(&self.selector_ma100)
            .next()
            .ok_or_else(|| anyhow::anyhow!("Failed to find MA100"))?
            .inner_html()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();
        let ma100 = ma100_str
            .parse()
            .context(format!("Failed to parse {:?}", ma100_str))?;

        Ok(MarketModuleState {
            val,
            delta_prct,
            ma50,
            ma100,
        })
    }
}

impl RenderablePolybarModule for MarketModule {
    type State = Option<MarketModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if let Some(prev_state) = prev_state {
            sleep(match prev_state {
                // Nominal
                Some(_) => Duration::from_secs(60 * 5),
                // Error occured
                None => Duration::from_secs(5),
            });
        }
        loop {
            let did_wait_mode = self.env.wait_runtime_mode(RuntimeMode::Unrestricted);
            match prev_state {
                Some(None) => break,
                None => break,
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
                        if state.ma50 >= state.ma100 {
                            ""
                        } else {
                            ""
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
            None => markup::style("", Some(theme::Color::Attention), None, None, None),
        }
    }
}

#[cfg(test)]
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
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
