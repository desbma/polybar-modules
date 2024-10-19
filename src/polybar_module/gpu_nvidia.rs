use std::{
    io::{BufRead as _, BufReader, ErrorKind},
    os::fd::AsRawFd as _,
    process::{Child, ChildStdout, Command, Stdio},
    thread::sleep,
    time::Duration,
};

use crate::{markup, polybar_module::RenderablePolybarModule, theme};

pub(crate) struct GpuNvidiaModule {
    _proc: Child,
    poller: mio::Poll,
    proc_output: BufReader<ChildStdout>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct GpuNvidiaModuleState {
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
    pub(crate) fn new() -> anyhow::Result<Self> {
        let mut proc = Command::new("nvidia-smi")
            .args([
                "-l", "1",
                "--format=csv,noheader,nounits",
                "--query-gpu=memory.used,memory.total,clocks.current.graphics,clocks.current.memory,clocks_throttle_reasons.hw_slowdown,temperature.gpu,power.draw"
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let poller = mio::Poll::new()?;

        let stdout = proc.stdout.take().unwrap();
        poller.registry().register(
            &mut mio::unix::SourceFd(&stdout.as_raw_fd()),
            mio::Token(0),
            mio::Interest::READABLE,
        )?;

        let proc_output = BufReader::new(stdout);

        Ok(Self {
            _proc: proc,
            poller,
            proc_output,
        })
    }

    fn try_update(&mut self) -> anyhow::Result<GpuNvidiaModuleState> {
        // Get output
        let mut output = String::new();
        let count = self.proc_output.read_line(&mut output)?;
        anyhow::ensure!(count > 0, "process exited");

        // Parse output
        let mut tokens = output.trim_end().split(',').map(str::trim_start);
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
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
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
        markup::style(
            icons[icons.len() - 1].0,
            Some(icons[icons.len() - 1].1.clone()),
            None,
            None,
            None,
        )
    }
}

impl RenderablePolybarModule for GpuNvidiaModule {
    type State = Option<GpuNvidiaModuleState>;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        if prev_state.is_none() {
            sleep(Duration::from_secs(1));
        } else {
            let mut poller_events = mio::Events::with_capacity(1);
            log::trace!("Waiting for stdout data");
            loop {
                let poll_res = self.poller.poll(&mut poller_events, None);
                if let Err(e) = &poll_res {
                    if e.kind() == ErrorKind::Interrupted {
                        // Ignore error, can occur on return from hibernation
                        continue;
                    }
                }
                poll_res.unwrap();
                log::trace!("Poll returned with events {:?}", poller_events);
                if poller_events.iter().any(mio::event::Event::is_readable) {
                    break;
                }
            }
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
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
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
                let mem_prct = 100.0 * f32::from(state.mem_used) / f32::from(state.mem_total);
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
#[expect(clippy::shadow_unrelated)]
mod tests {
    use super::*;

    #[test]
    fn test_render() {
        let module = GpuNvidiaModule::new().unwrap();

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

        let state = Some(GpuNvidiaModuleState {
            mem_used: 3963,
            mem_total: 4040,
            freq_graphics: 1600,
            freq_mem: 2000,
            throttle: false,
            temp: 70,
            power_draw: 200,
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 98% %{F#dc322f}█%{F-} 1600+2000MHz %{u#dc322f}%{+u}%{F#dc322f}70°C%{F-}%{-u} 200W"
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
