use std::process::{Command, Stdio};

use crate::markup;
use crate::polybar_module::{PolybarModuleEnv, RenderablePolybarModule, RuntimeMode};
use crate::theme;

pub struct DebianUpdatesModule {
    env: PolybarModuleEnv,
    debian_relase_codename: String,
}

#[derive(Debug, PartialEq)]
pub struct DebianUpdatesModuleState {
    update_count: usize,
    security_update_count: usize,
}

impl DebianUpdatesModule {
    pub fn new() -> anyhow::Result<DebianUpdatesModule> {
        let env = PolybarModuleEnv::new();

        // Run lsb_release
        let output = Command::new("lsb_release")
            .args(&["-sc"])
            .stderr(Stdio::null())
            .output()?;
        if !output.status.success() {
            anyhow::bail!("lsb_release invocation failed");
        }

        // Parse output
        let debian_relase_codename = String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string();

        Ok(DebianUpdatesModule {
            env,
            debian_relase_codename,
        })
    }

    fn try_update(&mut self) -> anyhow::Result<DebianUpdatesModuleState> {
        // Run apt
        let output = Command::new("apt")
            .args(&["list", "--upgradable"])
            .env("LANG", "C")
            .stderr(Stdio::null())
            .output()?;
        if !output.status.success() {
            anyhow::bail!("apt invocation failed");
        }

        // Parse output
        let output_str = String::from_utf8_lossy(&output.stdout);
        let updates: Vec<&str> = output_str
            .lines()
            .filter(|l| l.contains('['))
            .map(|l| l.split('/').next().unwrap())
            .collect();

        let security_update_count = if !updates.is_empty() {
            // Run debsecan
            let output = Command::new("debsecan")
                .args(&[
                    "--only-fixed",
                    &format!("--suite={}", self.debian_relase_codename),
                    "--format=packages",
                ])
                .env("LANG", "C")
                .stderr(Stdio::null())
                .output()?;
            if !output.status.success() {
                anyhow::bail!("debsecan invocation failed");
            }

            // Parse output
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|p| updates.contains(p))
                .count()
        } else {
            0
        };

        Ok(DebianUpdatesModuleState {
            update_count: updates.len(),
            security_update_count,
        })
    }
}

impl RenderablePolybarModule for DebianUpdatesModule {
    type State = Option<DebianUpdatesModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if let Some(prev_state) = prev_state {
            std::thread::sleep(match prev_state {
                // Nominal
                Some(_) => std::time::Duration::from_secs(60),
                // Error occured
                None => std::time::Duration::from_secs(5),
            });
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