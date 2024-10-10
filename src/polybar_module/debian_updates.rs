use std::{
    process::{Command, Stdio},
    thread::sleep,
    time::Duration,
};

use anyhow::Context;
use backoff::backoff::Backoff;

use crate::{
    markup,
    polybar_module::{NetworkMode, PolybarModuleEnv, RenderablePolybarModule},
    theme,
};

pub(crate) struct DebianUpdatesModule {
    env: PolybarModuleEnv,
    debian_relase_codename: String,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct DebianUpdatesModuleState {
    update_count: usize,
    security_update_count: usize,
}

impl DebianUpdatesModule {
    pub(crate) fn new() -> anyhow::Result<Self> {
        let env = PolybarModuleEnv::new();

        // Run lsb_release
        let output = Command::new("lsb_release")
            .args(["-sc"])
            .stderr(Stdio::null())
            .output()?;
        output
            .status
            .exit_ok()
            .context("lsb_release exited with error")?;

        // Parse output
        let debian_relase_codename = String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_owned();
        // if debian_relase_codename == "bullseye" {
        //     // Debian, sigh...
        //     debian_relase_codename = String::from("sid");
        // }

        Ok(Self {
            env,
            debian_relase_codename,
        })
    }

    fn try_update(&mut self) -> anyhow::Result<DebianUpdatesModuleState> {
        // Run apt
        let output_apt = Command::new("apt")
            .args(["list", "--upgradable"])
            .env("LANG", "C")
            .stderr(Stdio::null())
            .output()?;
        output_apt
            .status
            .exit_ok()
            .context("apt exited with error")?;

        // Parse output
        let output_apt_str = String::from_utf8_lossy(&output_apt.stdout);
        let updates: Vec<&str> = output_apt_str
            .lines()
            .filter(|l| l.contains('['))
            .map(|l| {
                l.split('/')
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("Failed to parse apt output"))
            })
            .collect::<Result<_, _>>()?;

        let security_update_count = if updates.is_empty() {
            0
        } else {
            // Run debsecan
            let output_debsecan = Command::new("debsecan")
                .args([
                    "--only-fixed",
                    &format!("--suite={}", self.debian_relase_codename),
                    "--format=packages",
                ])
                .env("LANG", "C")
                .stderr(Stdio::null())
                .output()?;
            output_debsecan
                .status
                .exit_ok()
                .context("debsecan exited with error")?;

            // Parse output
            String::from_utf8_lossy(&output_debsecan.stdout)
                .lines()
                .filter(|p| updates.contains(p))
                .count()
        };

        Ok(DebianUpdatesModuleState {
            update_count: updates.len(),
            security_update_count,
        })
    }
}

impl RenderablePolybarModule for DebianUpdatesModule {
    type State = Option<DebianUpdatesModuleState>;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        if let Some(prev_state) = prev_state {
            let sleep_duration = match prev_state {
                // Nominal
                Some(_) => {
                    self.env.network_error_backoff.reset();
                    Duration::from_secs(60 * 3)
                }
                // Error occured
                None => self.env.network_error_backoff.next_backoff().unwrap(),
            };
            sleep(sleep_duration);
        }
        self.env.wait_network_mode(&NetworkMode::Unrestricted);
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
                if state.update_count == 0 {
                    String::new()
                } else {
                    let mut r = format!(
                        "{} {}",
                        markup::style("", Some(theme::Color::MainIcon), None, None, None),
                        state.update_count
                    );
                    if state.security_update_count > 0 {
                        r += &markup::style(
                            &format!("({})", state.security_update_count),
                            Some(theme::Color::Attention),
                            None,
                            None,
                            None,
                        );
                    }
                    r
                }
            }
            None => markup::style("", Some(theme::Color::Attention), None, None, None),
        }
    }
}

#[cfg(test)]
#[allow(clippy::shadow_unrelated)]
mod tests {
    use super::*;

    #[test]
    fn test_render() {
        let module = DebianUpdatesModule::new().unwrap();

        let state = Some(DebianUpdatesModuleState {
            update_count: 0,
            security_update_count: 0,
        });
        assert_eq!(module.render(&state), "");

        let state = Some(DebianUpdatesModuleState {
            update_count: 12,
            security_update_count: 0,
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 12");

        let state = Some(DebianUpdatesModuleState {
            update_count: 12,
            security_update_count: 2,
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 12%{F#cb4b16}(2)%{F-}"
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
