use std::{collections::HashMap, sync::LazyLock, thread::sleep, time::Duration};

use backon::BackoffBuilder as _;

use crate::{
    markup,
    polybar_module::{
        NetworkMode, PolybarModuleEnv, RenderablePolybarModule, TCP_REMOTE_TIMEOUT,
        wait_network_ready,
    },
    theme::{self, ICON_WARNING},
};

pub(crate) struct WttrModule {
    client: ureq::Agent,
    url: String,
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
    pub(crate) fn new(location: Option<&String>) -> Self {
        let env = PolybarModuleEnv::new();
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
        let url = format!(
            "https://wttr.in/{}?format=%c/%t",
            location.map_or("", String::as_str)
        );
        Self { client, url, env }
    }

    fn try_update(&mut self) -> anyhow::Result<WttrModuleState> {
        let response = self.client.get(&self.url).call()?;
        anyhow::ensure!(
            response.status().is_success(),
            "HTTP response {}",
            response.status(),
        );
        let text = response.into_body().read_to_string()?;
        log::debug!("{text:?}");

        let mut tokens = text.split('/').map(str::trim);

        let sky_str = tokens
            .next()
            .ok_or_else(|| anyhow::anyhow!("Error parsing string {text:?}"))?;
        let sky = ICONS
            .get(sky_str)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("Error parsing string {text:?}"))?;

        let temp_str = tokens
            .next()
            .ok_or_else(|| anyhow::anyhow!("Error parsing string {text:?}"))?
            .split('°')
            .next()
            .ok_or_else(|| anyhow::anyhow!("Error parsing string {text:?}"))?;
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
                    self.env.network_error_backoff = self.env.network_error_backoff_builder.build();
                    Duration::from_mins(5)
                }
                // Error occured
                None => self.env.network_error_backoff.next().unwrap(),
            };
            sleep(sleep_duration);
        } else {
            wait_network_ready().unwrap();
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
                    markup::Markup::new(state.sky)
                        .fg(theme::Color::MainIcon)
                        .into_string(),
                    state.temp
                )
            }
            None => markup::Markup::new(ICON_WARNING)
                .fg(theme::Color::Attention)
                .into_string(),
        }
    }
}

#[cfg(test)]
#[expect(clippy::shadow_unrelated)]
mod tests {
    use super::*;

    #[test]
    fn test_render() {
        let module = WttrModule::new(None);

        let state = Some(WttrModuleState {
            sky: "", temp: 15
        });
        assert_eq!(module.render(&state), "%{F#f1e9d2}%{F-} 15°C");

        let state = None;
        assert_eq!(module.render(&state), "%{F#d56500}%{F-}");
    }
}
