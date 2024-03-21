use std::{collections::HashMap, thread::sleep, time::Duration};

use backoff::backoff::Backoff;
use lazy_static::lazy_static;

use crate::{
    markup,
    polybar_module::{NetworkMode, PolybarModuleEnv, RenderablePolybarModule, TCP_REMOTE_TIMEOUT},
    theme,
};

pub struct WttrModule {
    client: reqwest::blocking::Client,
    req: reqwest::blocking::Request,
    env: PolybarModuleEnv,
}

#[derive(Debug, Eq, PartialEq)]
pub struct WttrModuleState {
    sky: &'static str,
    temp: i8,
}

lazy_static! {
    static ref ICONS: HashMap<&'static str, &'static str> = {
        let mut m = HashMap::new();
        m.insert("✨", "?");  // unknown
        m.insert("☁️", "");  // Cloudy
        m.insert("🌫", "");  // Fog
        m.insert("🌧", "");  // HeavyRain
        m.insert("🌧", "");  // HeavyShowers
        m.insert("❄️", "");  // HeavySnow
        m.insert("❄️", "");  // HeavySnowShowers
        m.insert("🌦", "");  // LightRain
        m.insert("🌦", "");  // LightShowers
        m.insert("🌧", "");  // LightSleet
        m.insert("🌧", "");  // LightSleetShowers
        m.insert("🌨", "");  // LightSnow
        m.insert("🌨", "");  // LightSnowShowers
        m.insert("⛅️", "");  // PartlyCloudy
        m.insert("☀️", "");  // Sunny
        m.insert("🌩", "");  // ThunderyHeavyRain
        m.insert("⛈", "");  // ThunderyShowers
        m.insert("⛈", "");  // ThunderySnowShowers
        m.insert("☁️", "");
        m
    };
}

impl WttrModule {
    pub fn new(location: Option<String>) -> anyhow::Result<Self> {
        let env = PolybarModuleEnv::new();
        let client = reqwest::blocking::Client::builder()
            .timeout(TCP_REMOTE_TIMEOUT)
            .build()?;
        let url = &format!(
            "https://wttr.in/{}?format=%c/%t",
            location.as_ref().unwrap_or(&"".to_string())
        );
        let req = client.get(url).build()?;
        Ok(Self { client, req, env })
    }

    fn try_update(&mut self) -> anyhow::Result<WttrModuleState> {
        let text = self
            .client
            .execute(self.req.try_clone().unwrap())?
            .error_for_status()?
            .text()?;
        log::debug!("{:?}", text);

        let mut tokens = text.split('/').map(|s| s.trim());

        let sky_str = tokens
            .next()
            .ok_or_else(|| anyhow::anyhow!("Error parsing string {:?}", text))?;
        let sky = ICONS
            .get(sky_str)
            .ok_or_else(|| anyhow::anyhow!("Error parsing string {:?}", text))?;

        let temp_str = tokens
            .next()
            .ok_or_else(|| anyhow::anyhow!("Error parsing string {:?}", text))?
            .split('°')
            .next()
            .ok_or_else(|| anyhow::anyhow!("Error parsing string {:?}", text))?;
        let temp = temp_str.parse()?;

        Ok(WttrModuleState { sky, temp })
    }
}

impl RenderablePolybarModule for WttrModule {
    type State = Option<WttrModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if let Some(prev_state) = prev_state {
            let sleep_duration = match prev_state {
                // Nominal
                Some(_) => {
                    self.env.network_error_backoff.reset();
                    Duration::from_secs(60 * 5)
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
                    "{} {}°C",
                    markup::style(state.sky, Some(theme::Color::MainIcon), None, None, None),
                    state.temp
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
        let module = WttrModule::new(None).unwrap();

        let state = Some(WttrModuleState {
            sky: "", temp: 15
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 15°C");

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
