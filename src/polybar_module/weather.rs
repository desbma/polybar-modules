use std::{thread::sleep, time::Duration};

use backon::BackoffBuilder as _;

use crate::{
    config::WeatherModuleConfig,
    markup,
    polybar_module::{
        NetworkMode, PolybarModuleEnv, RenderablePolybarModule, TCP_REMOTE_TIMEOUT,
        wait_network_ready,
    },
    theme::{self, ICON_WARNING},
};

const WIND_STRONG_KMH: f64 = 40.0;
const WIND_GUST_STRONG_KMH: f64 = 60.0;

pub(crate) struct WeatherModule {
    client: ureq::Agent,
    url: String,
    env: PolybarModuleEnv,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct WeatherModuleState {
    icon: &'static str,
    temp: i16,
}

#[derive(serde::Deserialize)]
struct OpenMeteoResponse {
    current: OpenMeteoCurrent,
}

#[derive(serde::Deserialize)]
struct OpenMeteoCurrent {
    temperature_2m: f64,
    weather_code: u8,
    is_day: u8,
    wind_speed_10m: f64,
    wind_gusts_10m: f64,
}

#[expect(clippy::match_same_arms)]
fn weather_icon(
    code: u8,
    is_day: bool,
    wind_speed: f64,
    wind_gusts: f64,
) -> anyhow::Result<&'static str> {
    let windy = wind_speed >= WIND_STRONG_KMH || wind_gusts >= WIND_GUST_STRONG_KMH;
    Ok(match (code, is_day, windy) {
        (0 | 1, _, true) => "",      // nf-weather-windy
        (0 | 1, true, false) => "",  // nf-weather-day_sunny
        (0 | 1, false, false) => "", // nf-weather-night_clear
        (2, true, _) => "",          // nf-weather-day_cloudy
        (2, false, _) => "",         // nf-weather-night_alt_cloudy
        (3, ..) => "󰖐",               // nf-md-weather_cloudy
        (45 | 48, ..) => "󰖑",         // nf-md-weather_fog
        (51 | 53 | 55, ..) => "󰖗",    // nf-md-weather_rainy
        (56 | 57, ..) => "󰙿",         // nf-md-weather_snowy_rainy
        (61 | 63, ..) => "󰖗",         // nf-md-weather_rainy
        (65, ..) => "󰖖",              // nf-md-weather_pouring
        (66 | 67, ..) => "󰙿",         // nf-md-weather_snowy_rainy
        (71 | 73, ..) => "󰖘",         // nf-md-weather_snowy
        (75, ..) => "󰼶",              // nf-md-weather_snowy_heavy
        (77, ..) => "󰖘",              // nf-md-weather_snowy
        (80 | 81, true, _) => "",    // nf-weather-day_showers
        (80 | 81, false, _) => "",   // nf-weather-night_alt_showers
        (82, ..) => "󰖖",              // nf-md-weather_pouring
        (85, ..) => "󰖘",              // nf-md-weather_snowy
        (86, ..) => "󰼶",              // nf-md-weather_snowy_heavy
        (95, ..) => "󰙾",              // nf-md-weather_lightning_rainy
        (96 | 99, ..) => "󰖒",         // nf-md-weather_hail
        _ => anyhow::bail!("Unknown WMO weather code: {code}"),
    })
}

impl WeatherModule {
    pub(crate) fn new(cfg: &WeatherModuleConfig) -> Self {
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
            "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current=temperature_2m,weather_code,is_day,wind_speed_10m,wind_gusts_10m",
            cfg.latitude, cfg.longitude,
        );
        Self { client, url, env }
    }

    fn try_update(&mut self) -> anyhow::Result<WeatherModuleState> {
        let response = self.client.get(&self.url).call()?;
        anyhow::ensure!(
            response.status().is_success(),
            "HTTP response {}",
            response.status(),
        );
        let text = response.into_body().read_to_string()?;
        log::debug!("{text:?}");

        Self::parse(&text)
    }

    fn parse(text: &str) -> anyhow::Result<WeatherModuleState> {
        let response: OpenMeteoResponse = serde_json::from_str(text)?;
        let current = response.current;
        let icon = weather_icon(
            current.weather_code,
            current.is_day != 0,
            current.wind_speed_10m,
            current.wind_gusts_10m,
        )?;
        #[expect(clippy::cast_possible_truncation)]
        let temp = current.temperature_2m.round() as i16;
        Ok(WeatherModuleState { icon, temp })
    }
}

impl RenderablePolybarModule for WeatherModule {
    type State = Option<WeatherModuleState>;

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
                    markup::Markup::new(state.icon)
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

    fn test_cfg() -> WeatherModuleConfig {
        WeatherModuleConfig {
            latitude: 48.8566,
            longitude: 2.3522,
        }
    }

    #[test]
    #[expect(clippy::float_cmp)]
    fn test_config_deserialization() {
        let toml = "
[module.weather]
latitude = 48.8566
longitude = 2.3522
";
        let config: crate::config::Config = toml::from_str(toml).unwrap();
        let weather_cfg = config.module.unwrap().weather.unwrap();
        assert_eq!(weather_cfg.latitude, 48.8566);
        assert_eq!(weather_cfg.longitude, 2.3522);
    }

    #[test]
    fn test_parse_response() {
        let json = r#"{
            "latitude": 48.86,
            "longitude": 2.3399963,
            "current": {
                "time": "2024-01-15T12:00",
                "interval": 900,
                "temperature_2m": 14.6,
                "weather_code": 3,
                "is_day": 1,
                "wind_speed_10m": 15.5,
                "wind_gusts_10m": 25.3
            }
        }"#;
        let state = WeatherModule::parse(json).unwrap();
        assert_eq!(
            state,
            WeatherModuleState {
                icon: "󰖐",
                temp: 15,
            }
        );
    }

    #[test]
    fn test_parse_temperature_rounding() {
        let json = r#"{
            "current": {
                "temperature_2m": 14.4,
                "weather_code": 0,
                "is_day": 1,
                "wind_speed_10m": 0.0,
                "wind_gusts_10m": 0.0
            }
        }"#;
        assert_eq!(WeatherModule::parse(json).unwrap().temp, 14);

        let json = r#"{
            "current": {
                "temperature_2m": -3.5,
                "weather_code": 0,
                "is_day": 1,
                "wind_speed_10m": 0.0,
                "wind_gusts_10m": 0.0
            }
        }"#;
        assert_eq!(WeatherModule::parse(json).unwrap().temp, -4);
    }

    #[test]
    fn test_weather_icon_day_night() {
        assert_eq!(weather_icon(0, true, 0.0, 0.0).unwrap(), "");
        assert_eq!(weather_icon(0, false, 0.0, 0.0).unwrap(), "");
        assert_eq!(weather_icon(1, true, 0.0, 0.0).unwrap(), "");
        assert_eq!(weather_icon(1, false, 0.0, 0.0).unwrap(), "");
        assert_eq!(weather_icon(2, true, 0.0, 0.0).unwrap(), "");
        assert_eq!(weather_icon(2, false, 0.0, 0.0).unwrap(), "");
        assert_eq!(weather_icon(80, true, 0.0, 0.0).unwrap(), "");
        assert_eq!(weather_icon(80, false, 0.0, 0.0).unwrap(), "");
    }

    #[test]
    fn test_weather_icon_wind_override() {
        assert_eq!(weather_icon(0, true, 50.0, 0.0).unwrap(), "");
        assert_eq!(weather_icon(0, false, 0.0, 70.0).unwrap(), "");
        assert_eq!(weather_icon(0, true, 39.0, 59.0).unwrap(), "");
        assert_eq!(weather_icon(3, true, 100.0, 100.0).unwrap(), "󰖐");
    }

    #[test]
    fn test_weather_icon_all_codes() {
        let codes = [
            (3_u8, "󰖐"),
            (45, "󰖑"),
            (48, "󰖑"),
            (51, "󰖗"),
            (53, "󰖗"),
            (55, "󰖗"),
            (56, "󰙿"),
            (57, "󰙿"),
            (61, "󰖗"),
            (63, "󰖗"),
            (65, "󰖖"),
            (66, "󰙿"),
            (67, "󰙿"),
            (71, "󰖘"),
            (73, "󰖘"),
            (75, "󰼶"),
            (77, "󰖘"),
            (82, "󰖖"),
            (85, "󰖘"),
            (86, "󰼶"),
            (95, "󰙾"),
            (96, "󰖒"),
            (99, "󰖒"),
        ];
        for (code, expected) in codes {
            assert_eq!(weather_icon(code, true, 0.0, 0.0).unwrap(), expected);
        }
    }

    #[test]
    fn test_weather_icon_unknown_code() {
        assert!(weather_icon(123, true, 0.0, 0.0).is_err());
    }

    #[test]
    fn test_render() {
        let module = WeatherModule::new(&test_cfg());

        let state = Some(WeatherModuleState {
            icon: "󰖙",
            temp: 15,
        });
        assert_eq!(module.render(&state), "%{F#f1e9d2}󰖙%{F-} 15°C");

        let state = None;
        assert_eq!(module.render(&state), "%{F#d56500}%{F-}");
    }
}
