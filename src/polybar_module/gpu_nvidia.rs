use crate::markup;
use crate::polybar_module::StatefulPolybarModule;
use crate::theme;

pub struct GpuNvidiaModule {}

#[derive(Debug, PartialEq)]
pub struct GpuNvidiaModuleState {
    mem_used: u16,
    mem_total: u16,
    freq_graphics: u16,
    freq_mem: u16,
    throttle: bool,
    temp: u8,
    power_draw: u16,
}

const OVERHEAT_TEMP_THRESHOLD: u8 = 70;

impl GpuNvidiaModule {
    pub fn new() -> GpuNvidiaModule {
        GpuNvidiaModule {}
    }

    fn try_update(&mut self) -> anyhow::Result<GpuNvidiaModuleState> {
        // Run nvidia-smi
        let output = std::process::Command::new("nvidia-smi")
            .args(&[
                "--format=csv,noheader,nounits",
                "--query-gpu=memory.used,memory.total,clocks.current.graphics,clocks.current.memory,clocks_throttle_reasons.hw_slowdown,temperature.gpu,power.draw"
            ])
            .stderr(std::process::Stdio::null())
            .output()?;
        if !output.status.success() {
            anyhow::bail!("nvidia-smi invocation failed");
        }

        // Parse output
        let output_str = String::from_utf8_lossy(&output.stdout);
        let mut tokens = output_str.trim_end().split(',').map(|s| s.trim_start());
        let parse_err_str = "Failed to parse nvidia-smi output";
        let mem_used = tokens
            .next()
            .ok_or_else(|| anyhow::anyhow!(parse_err_str))?
            .parse()?;
        let mem_total = tokens
            .next()
            .ok_or_else(|| anyhow::anyhow!(parse_err_str))?
            .parse()?;
        let freq_graphics = tokens
            .next()
            .ok_or_else(|| anyhow::anyhow!(parse_err_str))?
            .parse()?;
        let freq_mem = tokens
            .next()
            .ok_or_else(|| anyhow::anyhow!(parse_err_str))?
            .parse()?;
        let throttle = tokens
            .next()
            .ok_or_else(|| anyhow::anyhow!(parse_err_str))?
            == "Active";
        let temp = tokens
            .next()
            .ok_or_else(|| anyhow::anyhow!(parse_err_str))?
            .parse()?;
        let power_draw = tokens
            .next()
            .ok_or_else(|| anyhow::anyhow!(parse_err_str))?
            .parse::<f32>()? as u16;

        Ok(GpuNvidiaModuleState {
            mem_used,
            mem_total,
            freq_graphics,
            freq_mem,
            throttle,
            temp,
            power_draw,
        })
    }

    fn ramp_prct(prct: u8) -> String {
        let icons: [(&str, theme::Color); 8] = [
            ("▁", theme::Color::Good),
            ("▂", theme::Color::Good),
            ("▃", theme::Color::Good),
            ("▄", theme::Color::Notice),
            ("▅", theme::Color::Notice),
            ("▆", theme::Color::Attention),
            ("▇", theme::Color::Attention),
            ("█", theme::Color::Critical),
        ];
        for (i, (icon, color)) in icons.iter().enumerate() {
            if prct as usize <= 100 / icons.len() * (i + 1) {
                return markup::style(icon, Some(color.to_owned()), None, None, None);
            }
        }
        unreachable!();
    }
}

impl StatefulPolybarModule for GpuNvidiaModule {
    type State = Option<GpuNvidiaModuleState>;

    fn wait_update(&mut self, first_update: bool) {
        if !first_update {
            std::thread::sleep(std::time::Duration::from_secs(1));
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
                let temp_str = if state.throttle || state.temp >= OVERHEAT_TEMP_THRESHOLD {
                    markup::style(
                        &format!("{}°C", state.temp),
                        Some(theme::Color::Critical),
                        Some(theme::Color::Critical),
                        None,
                        None,
                    )
                } else {
                    format!("{}°C", state.temp)
                };
                let mem_prct = 100.0 * state.mem_used as f32 / state.mem_total as f32;
                format!(
                    "{} {:2.0}% {} {:4}+{:4}MHz {} {:3}W",
                    markup::style("", Some(theme::Color::MainIcon), None, None, None),
                    mem_prct,
                    Self::ramp_prct(mem_prct as u8),
                    state.freq_graphics,
                    state.freq_mem,
                    temp_str,
                    state.power_draw
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
        let module = GpuNvidiaModule::new();
        let state = Some(GpuNvidiaModuleState {
            mem_used: 200,
            mem_total: 4000,
            freq_graphics: 600,
            freq_mem: 800,
            throttle: false,
            temp: 40,
            power_draw: 20,
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-}  5% %{F#859900}▁%{F-}  600+ 800MHz 40°C  20W"
        );
        let state = Some(GpuNvidiaModuleState {
            mem_used: 3500,
            mem_total: 4000,
            freq_graphics: 1600,
            freq_mem: 2000,
            throttle: false,
            temp: 69,
            power_draw: 200,
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 88% %{F#dc322f}█%{F-} 1600+2000MHz 69°C 200W"
        );
        let state = Some(GpuNvidiaModuleState {
            mem_used: 3500,
            mem_total: 4000,
            freq_graphics: 1600,
            freq_mem: 2000,
            throttle: true,
            temp: 69,
            power_draw: 200,
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 88% %{F#dc322f}█%{F-} 1600+2000MHz %{u#dc322f}%{+u}%{F#dc322f}69°C%{F-}%{-u} 200W"
        );
        let state = Some(GpuNvidiaModuleState {
            mem_used: 3500,
            mem_total: 4000,
            freq_graphics: 1600,
            freq_mem: 2000,
            throttle: false,
            temp: 70,
            power_draw: 200,
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 88% %{F#dc322f}█%{F-} 1600+2000MHz %{u#dc322f}%{+u}%{F#dc322f}70°C%{F-}%{-u} 200W"
        );
        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
