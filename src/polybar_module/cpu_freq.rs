use std::{
    fs::{self, File},
    io::{Read as _, Seek as _},
    path::PathBuf,
    thread::sleep,
    time::Duration,
};

use crate::{
    markup,
    polybar_module::RenderablePolybarModule,
    theme::{self, ICON_WARNING},
};

pub(crate) struct CpuFreqModule {
    freq_range: (u32, u32),
    freq_files: Vec<File>,
}

#[derive(Debug, Eq, PartialEq)]
#[expect(clippy::struct_field_names)]
pub(crate) struct CpuFreqModuleState {
    min_freq: u32,
    max_freq: u32,
    avg_freq: u32,
}

impl CpuFreqModule {
    pub(crate) fn new() -> anyhow::Result<Self> {
        let dirs: Vec<PathBuf> =
            glob::glob("/sys/devices/system/cpu/cpu*/cpufreq/")?.collect::<Result<_, _>>()?;
        log::debug!("{} CPUs", dirs.len());

        let freq_files: Vec<File> = dirs
            .iter()
            .map(|p| p.join("scaling_cur_freq"))
            .map(File::open)
            .collect::<Result<_, _>>()?;
        assert_eq!(dirs.len(), freq_files.len());

        let freq_min: u32 = dirs
            .iter()
            .map(|p| p.join("scaling_min_freq"))
            .map(fs::read_to_string)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|s| s.trim_end().parse::<u32>())
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .min()
            .ok_or_else(|| anyhow::anyhow!("Unable to read minimum CPU frequency"))?;
        let freq_max: u32 = dirs
            .iter()
            .map(|p| p.join("scaling_max_freq"))
            .map(fs::read_to_string)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|s| s.trim_end().parse::<u32>())
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .max()
            .ok_or_else(|| anyhow::anyhow!("Unable to read maximum CPU frequency"))?;
        log::debug!("Frequency range: [{freq_min}, {freq_max}]");

        Ok(Self {
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
                #[expect(clippy::verbose_file_reads)]
                f.read_to_string(&mut s)?;
                f.rewind()?;
                Ok(s)
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|s| s.trim_end().parse::<u32>())
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .collect();
        let min_freq: u32 = *freqs
            .iter()
            .min()
            .ok_or_else(|| anyhow::anyhow!("Unable to read current CPU frequency"))?;
        let max_freq: u32 = *freqs.iter().max().unwrap();
        #[expect(clippy::cast_possible_truncation)]
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

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        if prev_state.is_some() {
            sleep(Duration::from_secs(1));
        }
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
                let freq_load = f64::from(100 * (state.avg_freq - self.freq_range.0))
                    / f64::from(self.freq_range.1 - self.freq_range.0);
                log::debug!("freq_load={freq_load}");
                markup::style(
                    &format!(
                        "{:.1}/{:.1}/{:.1} GHz",
                        f64::from(state.min_freq) / 1_000_000.0,
                        f64::from(state.avg_freq) / 1_000_000.0,
                        f64::from(state.max_freq) / 1_000_000.0
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
        let module = CpuFreqModule {
            freq_range: (1_000_000, 4_000_000),
            freq_files: vec![],
        };

        let state = Some(CpuFreqModuleState {
            min_freq: 1_000_000,
            max_freq: 4_000_000,
            avg_freq: 2_000_000,
        });
        assert_eq!(module.render(&state), "%{F#859900}1.0/2.0/4.0 GHz%{F-}");

        let state = Some(CpuFreqModuleState {
            min_freq: 1_000_000,
            max_freq: 4_000_000,
            avg_freq: 3_000_000,
        });
        assert_eq!(module.render(&state), "1.0/3.0/4.0 GHz");

        let state = Some(CpuFreqModuleState {
            min_freq: 1_000_000,
            max_freq: 4_000_000,
            avg_freq: 3_500_000,
        });
        assert_eq!(module.render(&state), "%{F#b58900}1.0/3.5/4.0 GHz%{F-}");

        let state = Some(CpuFreqModuleState {
            min_freq: 1_000_000,
            max_freq: 4_000_000,
            avg_freq: 4_500_000,
        });
        assert_eq!(module.render(&state), "%{F#cb4b16}1.0/4.5/4.0 GHz%{F-}");
    }
}
