use lazy_static::lazy_static;
use std::collections::HashMap;

use crate::markup;
use crate::polybar_module::{PolybarModuleEnv, RenderablePolybarModule, RuntimeMode};
use crate::theme;

pub struct WttrModule {
    location: Option<String>,
    env: PolybarModuleEnv,
}

#[derive(Debug, PartialEq)]
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
    pub fn new(location: Option<String>) -> WttrModule {
        let env = PolybarModuleEnv::new();
        WttrModule { location, env }
    }

    fn try_update(&mut self) -> anyhow::Result<WttrModuleState> {
        let url = &format!(
            "https://wttr.in/{}?format=%c/%t",
            self.location.as_ref().unwrap_or(&"".to_string())
        );
        log::debug!("{}", url);
        let text = reqwest::blocking::get(url)?.error_for_status()?.text()?;
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

    fn wait_update(&mut self, first_update: bool) {
        if !first_update {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
        self.env.wait_runtime_mode(RuntimeMode::Unrestricted);
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
        let module = WttrModule::new(None);
        let state = Some(WttrModuleState {
            sky: "", temp: 15
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 15°C");
        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
