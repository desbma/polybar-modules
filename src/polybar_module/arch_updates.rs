use std::fmt::Write;
use std::process::{Command, Stdio};

use crate::markup;
use crate::polybar_module::{NetworkMode, PolybarModuleEnv, RenderablePolybarModule};
use crate::theme;

pub struct ArchUpdatesModule {
    xdg_dirs: xdg::BaseDirectories,
    env: PolybarModuleEnv,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ArchUpdatesModuleState {
    repo_update_count: usize,
    repo_security_update_count: usize,
    aur_update_count: usize,
}

impl ArchUpdatesModule {
    pub fn new() -> anyhow::Result<ArchUpdatesModule> {
        let xdg_dirs = xdg::BaseDirectories::new()?;
        let env = PolybarModuleEnv::new();
        Ok(ArchUpdatesModule { xdg_dirs, env })
    }

    fn try_update(&mut self) -> anyhow::Result<ArchUpdatesModuleState> {
        // Run checkupdates
        let db_dir = self
            .xdg_dirs
            .find_cache_file("checkupdates")
            .ok_or_else(|| anyhow::anyhow!("Unable to find checkupdates database dir"))?;
        let output = Command::new("checkupdates")
            .env("CHECKUPDATES_DB", &db_dir)
            .stderr(Stdio::null())
            .output()?;
        // checkupdates returns non 0 when no updates is available

        // Parse output
        let output_str = String::from_utf8_lossy(&output.stdout);
        let repo_updates: Vec<String> = output_str
            .lines()
            .map(|l| {
                l.split(' ')
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("Failed to parse checkupdates output"))
                    .map(|s| s.to_string())
            })
            .collect::<Result<Vec<String>, _>>()?;

        let repo_security_update_count = if !repo_updates.is_empty() {
            // Run arch-audit
            let output = Command::new("arch-audit")
                .args([
                    "-u",
                    "-b",
                    db_dir
                        .to_str()
                        .ok_or_else(|| anyhow::anyhow!("Invalid database directory"))?,
                    "-f",
                    "%n",
                ])
                .env("TERM", "xterm") // workaround arch-audit bug
                .stderr(Stdio::null())
                .output()?;
            if !output.status.success() {
                anyhow::bail!("arch-audit invocation failed");
            }

            // Parse output
            let output_str = String::from_utf8_lossy(&output.stdout);
            output_str
                .lines()
                .filter(|l| repo_updates.contains(&l.to_string()))
                .count()
        } else {
            0
        };

        // Run arch-audit
        let output = Command::new("checkupdates-aur")
            .stderr(Stdio::null())
            .output()?;
        if !output.status.success() {
            anyhow::bail!("checkupdates-aur invocation failed");
        }

        // Parse output
        let output_str = String::from_utf8_lossy(&output.stdout);
        let aur_update_count = output_str.lines().count();

        Ok(ArchUpdatesModuleState {
            repo_update_count: repo_updates.len(),
            repo_security_update_count,
            aur_update_count,
        })
    }
}

impl RenderablePolybarModule for ArchUpdatesModule {
    type State = Option<ArchUpdatesModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if let Some(prev_state) = prev_state {
            std::thread::sleep(match prev_state {
                // Nominal
                Some(_) => std::time::Duration::from_secs(60 * 10),
                // Error occured
                None => std::time::Duration::from_secs(5),
            });
        }
        self.env.wait_network_mode(NetworkMode::Unrestricted);
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
                if state.repo_update_count == 0 && state.aur_update_count == 0 {
                    String::new()
                } else {
                    let mut r = format!(
                        "{} {}",
                        markup::style("", Some(theme::Color::MainIcon), None, None, None),
                        state.repo_update_count
                    );
                    if state.repo_security_update_count > 0 {
                        r += &markup::style(
                            &format!("({})", state.repo_security_update_count),
                            Some(theme::Color::Attention),
                            None,
                            None,
                            None,
                        );
                    }
                    if state.aur_update_count > 0 {
                        write!(r, "+{}", state.aur_update_count).unwrap();
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
        let module = ArchUpdatesModule::new().unwrap();

        let state = Some(ArchUpdatesModuleState {
            repo_update_count: 0,
            repo_security_update_count: 0,
            aur_update_count: 0,
        });
        assert_eq!(module.render(&state), "");

        let state = Some(ArchUpdatesModuleState {
            repo_update_count: 12,
            repo_security_update_count: 0,
            aur_update_count: 0,
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 12");

        let state = Some(ArchUpdatesModuleState {
            repo_update_count: 12,
            repo_security_update_count: 2,
            aur_update_count: 0,
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 12%{F#cb4b16}(2)%{F-}"
        );

        let state = Some(ArchUpdatesModuleState {
            repo_update_count: 12,
            repo_security_update_count: 2,
            aur_update_count: 3,
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 12%{F#cb4b16}(2)%{F-}+3"
        );

        let state = Some(ArchUpdatesModuleState {
            repo_update_count: 12,
            repo_security_update_count: 0,
            aur_update_count: 3,
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 12+3");

        let state = Some(ArchUpdatesModuleState {
            repo_update_count: 0,
            repo_security_update_count: 0,
            aur_update_count: 3,
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 0+3");

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
