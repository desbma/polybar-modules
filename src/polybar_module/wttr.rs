use lazy_static::lazy_static;
use std::collections::HashMap;

use crate::markup;
use crate::polybar_module::StatefulPolybarModule;
use crate::theme;

pub struct WttrModule {}

#[derive(Debug, PartialEq)]
pub struct WttrModuleState {
    sky: &'static str,
    temp: i8,
}

lazy_static! {
    /// This is an example for using doc comment attributes
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
    pub fn new() -> WttrModule {
        WttrModule {}
    }
}

impl StatefulPolybarModule for WttrModule {
    type State = WttrModuleState;

    fn wait_update(&mut self) {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }

    fn update(&mut self) -> Self::State {
        Self::State {
            sky: "☀️", temp: 25
        }
    }

    fn render(&self, state: &Self::State) -> String {
        format!(
            "{} {}°C",
            markup::style(state.sky, Some(theme::Color::MainIcon), None, None, None),
            state.temp
        )
    }
}
