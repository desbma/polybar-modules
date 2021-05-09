use std::error::Error;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use crate::markup;
use crate::polybar_module::RenderablePolybarModule;
use crate::theme;

pub struct BatteryMouseModule {
    sysfs_dirs: Vec<PathBuf>,
}

#[derive(Debug, PartialEq)]
pub struct BatteryMouseModuleState {
    levels: Vec<(String, Option<u8>)>,
}

impl BatteryMouseModule {
    pub fn new() -> BatteryMouseModule {
        let sysfs_dirs = match glob::glob("/sys/class/power_supply/hidpp_battery_*") {
            Err(_) => vec![],
            Ok(g) => g.filter_map(|e| e.ok()).collect(),
        };
        log::trace!("{:?}", sysfs_dirs);

        BatteryMouseModule { sysfs_dirs }
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
        let levels: Vec<(String, Option<u8>)> = self
            .sysfs_dirs
            .iter()
            .map(|p| {
                let mut capacity_filepath = p.to_owned();
                capacity_filepath.push("capacity_level");
                log::trace!("{:?}", capacity_filepath);
                // TODO read name only once
                let mut name_filepath = p.to_owned();
                name_filepath.push("model_name");
                log::trace!("{:?}", name_filepath);
                let mut capacity_str = String::new();
                File::open(&capacity_filepath)?.read_to_string(&mut capacity_str)?;
                capacity_str = capacity_str.trim_end().to_string();
                log::trace!("{:?}", capacity_str);
                let capacity = if capacity_str == "Full" {
                    Some(100)
                } else if capacity_str == "Unknown" {
                    None
                } else {
                    // TODO
                    Some(0)
                };
                let mut name_str = String::new();
                File::open(&name_filepath)?.read_to_string(&mut name_str)?;
                name_str = name_str.trim_end().to_string();
                name_str = name_str
                    .split(' ')
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("Failed to parse device name"))?
                    .to_string();
                Ok((name_str, capacity))
            })
            .filter_map(|d: Result<(String, Option<u8>), Box<dyn Error>>| d.ok())
            .collect();

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
