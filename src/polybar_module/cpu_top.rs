use std::io::BufRead;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use anyhow::Context;

use crate::markup;
use crate::polybar_module::RenderablePolybarModule;
use crate::theme;

pub struct CpuTopModule {
    max_len: Option<usize>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct CpuTopModuleState {
    cpu_prct: u32,
    process_name: String,
}

impl CpuTopModule {
    pub fn new(max_len: Option<usize>) -> anyhow::Result<Self> {
        Ok(Self { max_len })
    }

    fn try_update(&mut self) -> anyhow::Result<CpuTopModuleState> {
        // Run ps
        let output = Command::new("ps")
            .args([
                "-e",
                "--no-headers",
                "-o",
                "c,cmd:256,exe:256",
                "--sort",
                "-%cpu",
            ])
            .output()?;
        output.status.exit_ok().context("ps exited with error")?;

        // Parse output
        Self::parse_ps_output(&output.stdout)
    }

    fn parse_ps_output(stdout: &[u8]) -> anyhow::Result<CpuTopModuleState> {
        let proc_line = stdout
            .lines()
            .map_while(Result::ok)
            .map(|l| l.trim_start().to_owned())
            .find(|l| l.split(' ').nth(1) != Some("ps"))
            .ok_or_else(|| anyhow::anyhow!("Unexpected ps output"))?;
        let (cpu_prct_str, rest) = proc_line
            .split_once(' ')
            .ok_or_else(|| anyhow::anyhow!("Unexpected ps output"))?;
        let cpu_prct = cpu_prct_str.parse()?;
        let (cmd, exe) = rest
            .rsplit_once(' ')
            .map(|p| (p.0.trim_end(), p.1))
            .ok_or_else(|| anyhow::anyhow!("Unexpected ps output"))?;
        let process_name_match = if exe != "-" {
            exe.rsplit('/').next()
        } else if cmd.starts_with('[') && cmd.ends_with(']') {
            Some("[kthread]")
        } else {
            cmd.split(' ')
                .next()
                .ok_or_else(|| anyhow::anyhow!("Unexpected ps output"))?
                .rsplit('/')
                .next()
        };
        let process_name = process_name_match
            .ok_or_else(|| anyhow::anyhow!("Unexpected ps output"))?
            .to_string();

        Ok(CpuTopModuleState {
            cpu_prct,
            process_name,
        })
    }
}

impl RenderablePolybarModule for CpuTopModule {
    type State = Option<CpuTopModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        // TODO increase sleep delay if on battery?
        if let Some(prev_state) = prev_state {
            let sleep_duration = match prev_state {
                Some(state) if state.cpu_prct > 30 => Duration::from_secs(1),
                _ => Duration::from_secs(3),
            };
            sleep(sleep_duration);
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
        let max_proc_len = self.max_len.map(|v| v - 4);
        match state {
            Some(state) => markup::style(
                &format!(
                    "{: >2}% {}",
                    state.cpu_prct,
                    theme::pad(
                        &theme::ellipsis(&state.process_name, max_proc_len),
                        max_proc_len
                    ),
                ),
                if state.cpu_prct >= 90 {
                    Some(theme::Color::Attention)
                } else if state.cpu_prct >= 50 {
                    Some(theme::Color::Notice)
                } else {
                    None
                },
                None,
                None,
                None,
            ),
            None => markup::style("", Some(theme::Color::Attention), None, None, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ps_output() {
        let output = " 1 /usr/lib/firefox/firefox -contentproc -childID 6 -isForBrowser -prefsLen 55932 -prefMapSize 253692 -jsInitLen 235336 -parentBuildID 20231106235557 -greomni /usr/lib/firefox/omni.ja -appomni /usr/lib/firefox/browser/omni.ja -appDir /usr/lib/firefox/browser -";
        assert_eq!(
            CpuTopModule::parse_ps_output(output.as_bytes()).unwrap(),
            CpuTopModuleState {
                cpu_prct: 1,
                process_name: "firefox".to_string()
            }
        );

        let output = " 1 /usr/lib/Xorg :0 -seat seat0 -auth /run/lightdm/root/:0 -nolisten tcp vt7 -novtswitch                                                                                                                                                -";
        assert_eq!(
            CpuTopModule::parse_ps_output(output.as_bytes()).unwrap(),
            CpuTopModuleState {
                cpu_prct: 1,
                process_name: "Xorg".to_string()
            }
        );

        let output = " 0 polybar-modules network-status                                                                                                                                                /usr/bin/polybar-modules";
        assert_eq!(
            CpuTopModule::parse_ps_output(output.as_bytes()).unwrap(),
            CpuTopModuleState {
                cpu_prct: 0,
                process_name: "polybar-modules".to_string()
            }
        );

        let output = "99 ps -e --no-headers -o c,cmd:255,exe:255 --sort -%cpu                                                                                                                                                /usr/bin/ps\n 6 /usr/lib/thunderbird/thunderbird                                                                                                                                                -";
        assert_eq!(
            CpuTopModule::parse_ps_output(output.as_bytes()).unwrap(),
            CpuTopModuleState {
                cpu_prct: 6,
                process_name: "thunderbird".to_string()
            }
        );
    }

    #[test]
    fn test_render() {
        let module = CpuTopModule::new(Some(10)).unwrap();

        let state = Some(CpuTopModuleState {
            cpu_prct: 1,
            process_name: "bz".to_string(),
        });
        assert_eq!(module.render(&state), " 1%     bz");

        let state = Some(CpuTopModuleState {
            cpu_prct: 1,
            process_name: "bzzzzzzzzzzzzzzzz".to_string(),
        });
        assert_eq!(module.render(&state), " 1% bzzzz…");

        let state = Some(CpuTopModuleState {
            cpu_prct: 50,
            process_name: "bz".to_string(),
        });
        assert_eq!(module.render(&state), "%{F#b58900}50%     bz%{F-}");

        let state = Some(CpuTopModuleState {
            cpu_prct: 99,
            process_name: "bz".to_string(),
        });
        assert_eq!(module.render(&state), "%{F#cb4b16}99%     bz%{F-}");

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
