use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
    thread::sleep,
    time::Duration,
};

use backoff::backoff::Backoff as _;

use crate::{
    markup,
    polybar_module::{NetworkMode, PolybarModuleEnv, RenderablePolybarModule, TCP_REMOTE_TIMEOUT},
    theme::{self, ICON_WARNING},
};

pub(crate) struct WttrModule {
    req: ureq::Request,
    env: PolybarModuleEnv,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct WttrModuleState {
    sky: &'static str,
    temp: i8,
}

static ICONS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        ("✨", "?"), // unknown
        ("☁️", "󰖐"), // Cloudy
        ("🌫", "󰖑"),  // Fog
        ("🌧", "󰖖"),  // HeavyRain
        ("🌧", "󰖖"),  // HeavyShowers
        ("❄️", "󰼶"), // HeavySnow
        ("❄️", "󰼶"), // HeavySnowShowers
        ("🌦", "󰖗"),  // LightRain
        ("🌦", "󰖗"),  // LightShowers
        ("🌧", "󰖗"),  // LightSleet
        ("🌧", "󰖗"),  // LightSleetShowers
        ("🌨", "󰖘"),  // LightSnow
        ("🌨", "󰖘"),  // LightSnowShowers
        ("⛅️", "󰖕"), // PartlyCloudy
        ("☀️", "󰖙"), // Sunny
        ("🌩", "󰙾"),  // ThunderyHeavyRain
        ("⛈", "󰙾"),  // ThunderyShowers
        ("⛈", "󰙾"),  // ThunderySnowShowers
        ("☁️", "󰹮"),
    ])
});

impl WttrModule {
    pub(crate) fn new(location: Option<&String>) -> anyhow::Result<Self> {
        let env = PolybarModuleEnv::new();
        let client = ureq::AgentBuilder::new()
            .tls_connector(Arc::new(ureq::native_tls::TlsConnector::new()?))
            .timeout(TCP_REMOTE_TIMEOUT)
            .build();
        let url = format!(
            "https://wttr.in/{}?format=%c/%t",
            location.map_or("", String::as_str)
        );
        let req = client.get(&url);
        Ok(Self { req, env })
    }

    fn try_update(&mut self) -> anyhow::Result<WttrModuleState> {
        let response = self.req.clone().call()?;
        anyhow::ensure!(
            response.status() >= 200 && response.status() < 300,
            "HTTP response {}: {}",
            response.status(),
            response.status_text()
        );
        let text = response.into_string()?;
        log::debug!("{text:?}");

        let mut tokens = text.split('/').map(str::trim);

        let sky_str = tokens
            .next()
            .ok_or_else(|| anyhow::anyhow!("Error parsing string {:?}", text))?;
        let sky = ICONS
            .get(sky_str)
            .copied()
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

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
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
                    "{} {}°C",
                    markup::style(state.sky, Some(theme::Color::MainIcon), None, None, None),
                    state.temp
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
        let module = WttrModule::new(None).unwrap();

        let state = Some(WttrModuleState {
            sky: "", temp: 15
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 15°C");

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
