use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use crate::markup;
use crate::polybar_module::RenderablePolybarModule;
use crate::theme;

pub struct CpuFreqModule {
    freq_range: (u32, u32),
    freq_files: Vec<File>,
}

#[derive(Debug, PartialEq)]
pub struct CpuFreqModuleState {
    min_freq: u32,
    max_freq: u32,
    avg_freq: u32,
}

impl CpuFreqModule {
    pub fn new() -> anyhow::Result<CpuFreqModule> {
        let dirs: Vec<PathBuf> = glob::glob("/sys/devices/system/cpu/cpu*/cpufreq/")?
            .flatten()
            .collect();
        log::debug!("{} CPUs", dirs.len());

        let freq_files: Vec<File> = dirs
            .iter()
            .map(|p| p.join("scaling_cur_freq"))
            .map(File::open)
            .flatten()
            .collect();
        assert_eq!(dirs.len(), freq_files.len());

        let freq_min: u32 = dirs
            .iter()
            .map(|p| p.join("scaling_min_freq"))
            .map(File::open)
            .flatten()
            .map(|mut f| -> std::io::Result<String> {
                let mut s = String::new();
                f.read_to_string(&mut s)?;
                Ok(s)
            })
            .flatten()
            .map(|s| s.trim_end().parse::<u32>())
            .flatten()
            .min()
            .ok_or_else(|| anyhow::anyhow!("Unable to read minimum CPU frequency"))?;
        let freq_max: u32 = dirs
            .iter()
            .map(|p| p.join("scaling_max_freq"))
            .map(File::open)
            .flatten()
            .map(|mut f| -> std::io::Result<String> {
                let mut s = String::new();
                f.read_to_string(&mut s)?;
                Ok(s)
            })
            .flatten()
            .map(|s| s.trim_end().parse::<u32>())
            .flatten()
            .max()
            .ok_or_else(|| anyhow::anyhow!("Unable to read maximum CPU frequency"))?;
        log::debug!("Frequency range: [{}, {}]", freq_min, freq_max);

        Ok(CpuFreqModule {
            freq_range: (freq_min, freq_max),
            freq_files,
        })
    }

    fn try_update(&mut self) -> anyhow::Result<CpuFreqModuleState> {
        let freqs: Vec<u32> = self
            .freq_files
            .iter()
            .map(|mut f| -> std::io::Result<String> {
                let mut s = String::new();
                f.read_to_string(&mut s)?;
                f.seek(SeekFrom::Start(0))?;
                Ok(s)
            })
            .flatten()
            .map(|s| s.trim_end().parse::<u32>())
            .flatten()
            .collect();
        let min_freq: u32 = *freqs
            .iter()
            .min()
            .ok_or_else(|| anyhow::anyhow!("Unable to read current CPU frequency"))?;
        let max_freq: u32 = *freqs.iter().max().unwrap();
        let avg_freq: u32 = freqs.iter().sum::<u32>() / freqs.len() as u32;
        Ok(CpuFreqModuleState {
            min_freq,
            max_freq,
            avg_freq,
        })
    }
}

impl RenderablePolybarModule for CpuFreqModule {
    type State = Option<CpuFreqModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if prev_state.is_some() {
            sleep(Duration::from_secs(1));
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
                let freq_load = (100 * (state.avg_freq - self.freq_range.0)) as f64
                    / ((self.freq_range.1 - self.freq_range.0) as f64);
                log::debug!("freq_load={}", freq_load);
                markup::style(
                    &format!(
                        "{:.1}/{:.1}/{:.1} GHz",
                        state.min_freq as f64 / 1000000.0,
                        state.avg_freq as f64 / 1000000.0,
                        state.max_freq as f64 / 1000000.0
                    ),
                    if freq_load > 100.0 {
                        Some(theme::Color::Attention)
                    } else if freq_load > 80.0 {
                        Some(theme::Color::Notice)
                    } else if freq_load < 50.0 {
                        Some(theme::Color::Good)
                    } else {
                        None
                    },
                    None,
                    None,
                    None,
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
        let module = CpuFreqModule {
            freq_range: (1000000, 4000000),
            freq_files: vec![],
        };

        let state = Some(CpuFreqModuleState {
            min_freq: 1000000,
            max_freq: 4000000,
            avg_freq: 2000000,
        });
        assert_eq!(module.render(&state), "%{F#859900}1.0/2.0/4.0 GHz%{F-}");

        let state = Some(CpuFreqModuleState {
            min_freq: 1000000,
            max_freq: 4000000,
            avg_freq: 3000000,
        });
        assert_eq!(module.render(&state), "1.0/3.0/4.0 GHz");

        let state = Some(CpuFreqModuleState {
            min_freq: 1000000,
            max_freq: 4000000,
            avg_freq: 3500000,
        });
        assert_eq!(module.render(&state), "%{F#b58900}1.0/3.5/4.0 GHz%{F-}");

        let state = Some(CpuFreqModuleState {
            min_freq: 1000000,
            max_freq: 4000000,
            avg_freq: 4500000,
        });
        assert_eq!(module.render(&state), "%{F#cb4b16}1.0/4.5/4.0 GHz%{F-}");
    }
}
