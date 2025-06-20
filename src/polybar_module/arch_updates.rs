use std::{
    borrow::ToOwned,
    fmt::Write as _,
    process::{Command, Stdio},
    thread::sleep,
    time::Duration,
};

use anyhow::Context as _;
use backon::BackoffBuilder as _;

use crate::{
    markup,
    polybar_module::{NetworkMode, PolybarModuleEnv, RenderablePolybarModule},
    theme::{self, ICON_WARNING},
};

pub(crate) struct ArchUpdatesModule {
    xdg_dirs: xdg::BaseDirectories,
    env: PolybarModuleEnv,
    server_error_backoff_builder: backon::ExponentialBuilder,
    server_error_backoff: backon::ExponentialBackoff,
}

#[derive(Debug, Eq, PartialEq)]
#[expect(clippy::struct_field_names)]
pub(crate) struct ArchUpdatesModuleState {
    repo_update_count: usize,
    repo_security_update_count: usize,
    aur_update_count: usize,
}

impl ArchUpdatesModule {
    pub(crate) fn new() -> Self {
        let xdg_dirs = xdg::BaseDirectories::new();
        let env = PolybarModuleEnv::new();
        let server_error_backoff_builder = backon::ExponentialBuilder::default()
            .with_jitter()
            .with_factor(3.0)
            .with_min_delay(Duration::from_secs(15 * 60))
            .with_max_delay(Duration::from_secs(6 * 60 * 60))
            .without_max_times();
        let server_error_backoff = server_error_backoff_builder.build();
        Self {
            xdg_dirs,
            env,
            server_error_backoff_builder,
            server_error_backoff,
        }
    }

    fn try_update(&mut self) -> anyhow::Result<ArchUpdatesModuleState> {
        // Run checkupdates
        let db_dir = self
            .xdg_dirs
            .find_cache_file("checkupdates")
            .ok_or_else(|| anyhow::anyhow!("Unable to find checkupdates database dir"))?;
        let output_cu = Command::new("checkupdates")
            .env("CHECKUPDATES_DB", &db_dir)
            .stderr(Stdio::null())
            .output()?;
        // checkupdates returns non 0 when no update is available

        // Parse output
        let output_cu_str = String::from_utf8_lossy(&output_cu.stdout);
        let repo_updates: Vec<String> = output_cu_str
            .lines()
            .map(|l| {
                l.split(' ')
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("Failed to parse checkupdates output"))
                    .map(ToOwned::to_owned)
            })
            .collect::<Result<Vec<String>, _>>()?;

        let repo_security_update_count = if repo_updates.is_empty() {
            0
        } else {
            // Run arch-audit
            let output_audit = Command::new("arch-audit")
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
            output_audit
                .status
                .exit_ok()
                .context("arch-audit exited with error")?;

            // Parse output
            let output_audit_str = String::from_utf8_lossy(&output_audit.stdout);
            output_audit_str
                .lines()
                .filter(|l| repo_updates.contains(&(*l).to_owned()))
                .count()
        };

        // Run arch-audit
        let output_aur = Command::new("pikaur")
            .args(["-Qua"])
            .stderr(Stdio::null())
            .output()
            .or_else(|_| {
                Command::new("yay")
                    .args(["-Qua"])
                    .stderr(Stdio::null())
                    .output()
            })?;
        // output.status.exit_ok().context("yay exited with error")?;

        // Parse output
        let output_yay_str = String::from_utf8_lossy(&output_aur.stdout);
        let aur_update_count = output_yay_str.lines().count();

        Ok(ArchUpdatesModuleState {
            repo_update_count: repo_updates.len(),
            repo_security_update_count,
            aur_update_count,
        })
    }
}

pub(crate) const ICON_UPDATE: &str = "";

impl RenderablePolybarModule for ArchUpdatesModule {
    type State = Option<ArchUpdatesModuleState>;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        if let Some(prev_state) = prev_state {
            let sleep_duration = match prev_state {
                // Nominal
                Some(_) => {
                    self.server_error_backoff = self.server_error_backoff_builder.build();
                    Duration::from_secs(3 * 60 * 60)
                }
                // Error occured
                None => self.server_error_backoff.next().unwrap(),
            };
            sleep(sleep_duration);
        }
        self.env.wait_network_mode(&NetworkMode::Unrestricted);
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
                if state.repo_update_count == 0 && state.aur_update_count == 0 {
                    String::new()
                } else {
                    let mut r = format!(
                        "{} {}",
                        markup::style(ICON_UPDATE, Some(theme::Color::MainIcon), None, None, None),
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
        let module = ArchUpdatesModule::new();

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
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 12");

        let state = Some(ArchUpdatesModuleState {
            repo_update_count: 12,
            repo_security_update_count: 2,
            aur_update_count: 0,
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 12%{F#cb4b16}(2)%{F-}"
        );

        let state = Some(ArchUpdatesModuleState {
            repo_update_count: 12,
            repo_security_update_count: 2,
            aur_update_count: 3,
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 12%{F#cb4b16}(2)%{F-}+3"
        );

        let state = Some(ArchUpdatesModuleState {
            repo_update_count: 12,
            repo_security_update_count: 0,
            aur_update_count: 3,
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 12+3");

        let state = Some(ArchUpdatesModuleState {
            repo_update_count: 0,
            repo_security_update_count: 0,
            aur_update_count: 3,
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 0+3");

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
