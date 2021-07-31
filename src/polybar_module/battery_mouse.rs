use std::error::Error;
use std::fs::File;
use std::io::Read;
use std::thread::sleep;
use std::time::Duration;

use crate::markup;
use crate::polybar_module::RenderablePolybarModule;
use crate::theme;

pub struct BatteryMouseModule {}

#[derive(Debug, PartialEq)]
pub struct BatteryMouseModuleState {
    levels: Vec<(String, Option<u8>)>,
}

impl BatteryMouseModule {
    pub fn new() -> BatteryMouseModule {
        BatteryMouseModule {}
    }

    fn sysfs_capacity_level_to_prct(s: &str) -> Option<u8> {
        // See in kernel tree:
        // drivers/hid/hid-logitech-hidpp.c: hidpp_map_battery_level
        // drivers/power/supply/power_supply_sysfs.c: POWER_SUPPLY_CAPACITY_LEVEL_TEXT
        match s {
            "Full" => Some(100),
            "High" => Some(80),
            "Normal" => Some(60),
            "Low" => Some(30),
            "Critical" => Some(10),
            "Unknown" => None,
            v => panic!("Unexpected value: {:?}", v),
        }
    }
}

impl RenderablePolybarModule for BatteryMouseModule {
    type State = BatteryMouseModuleState;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if prev_state.is_some() {
            sleep(Duration::from_secs(5));
        }
    }

    fn update(&mut self) -> Self::State {
        let levels: Vec<(String, Option<u8>)> =
            match glob::glob("/sys/class/power_supply/hidpp_battery_*") {
                Err(_) => vec![],
                Ok(g) => g
                    .filter_map(|e| e.ok())
                    .map(|p| {
                        // Parse capacity
                        let mut capacity_filepath = p.to_owned();
                        capacity_filepath.push("capacity");
                        log::trace!("{:?}", capacity_filepath);
                        let capacity = match File::open(&capacity_filepath) {
                            Ok(mut f) => {
                                let mut capacity_str = String::new();
                                f.read_to_string(&mut capacity_str)?;
                                Some(capacity_str.trim_end().parse::<u8>()?)
                            }
                            Err(_) => {
                                let mut capacity_level_filepath = p.to_owned();
                                capacity_level_filepath.push("capacity_level");
                                log::trace!("{:?}", capacity_level_filepath);
                                let mut capacity_level_str = String::new();
                                File::open(&capacity_level_filepath)?
                                    .read_to_string(&mut capacity_level_str)?;
                                capacity_level_str = capacity_level_str.trim_end().to_string();
                                Self::sysfs_capacity_level_to_prct(&capacity_level_str)
                            }
                        };

                        // Parse model name
                        let mut name_filepath = p;
                        name_filepath.push("model_name");
                        log::trace!("{:?}", name_filepath);
                        let mut name_str = String::new();
                        File::open(&name_filepath)?.read_to_string(&mut name_str)?;
                        name_str = theme::shorten_model_name(name_str.trim_end());

                        Ok((name_str, capacity))
                    })
                    .filter_map(|d: Result<(String, Option<u8>), Box<dyn Error>>| d.ok())
                    .collect(),
            };

        BatteryMouseModuleState { levels }
    }

    fn render(&self, state: &Self::State) -> String {
        let mut fragments: Vec<String> = Vec::new();
        if !state.levels.is_empty() {
            fragments.push(markup::style(
                "",
                Some(theme::Color::MainIcon),
                None,
                None,
                None,
            ));
            for (name, level) in &state.levels {
                fragments.push(match level {
                    Some(level) => markup::style(
                        &format!("{} {}%", name, level),
                        if level < &40 {
                            Some(theme::Color::Attention)
                        } else if level < &50 {
                            Some(theme::Color::Notice)
                        } else {
                            None
                        },
                        None,
                        None,
                        None,
                    ),
                    None => format!("{} ?", name),
                });
            }
        }
        fragments.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render() {
        let module = BatteryMouseModule::new();

        let levels = vec![
            ("m0".to_string(), Some(100)),
            ("m1".to_string(), Some(50)),
            ("m2".to_string(), Some(49)),
            ("m3".to_string(), Some(30)),
            ("m4".to_string(), Some(29)),
            ("m5".to_string(), Some(5)),
            ("m6".to_string(), None),
        ];
        let state = BatteryMouseModuleState { levels };
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} m0 100% m1 50% %{F#b58900}m2 49%%{F-} %{F#cb4b16}m3 30%%{F-} %{F#cb4b16}m4 29%%{F-} %{F#cb4b16}m5 5%%{F-} m6 ?"
        );
    }
}
