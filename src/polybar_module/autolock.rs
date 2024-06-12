use std::{
    path::PathBuf,
    process::{Command, Stdio},
    thread::sleep,
    time::Duration,
};

use anyhow::Context;

use crate::{markup, polybar_module::RenderablePolybarModule, theme};

pub struct AutolockModule {
    xdg_dirs: xdg::BaseDirectories,
    signals: signal_hook::iterator::Signals,
}

#[derive(Debug, Eq, PartialEq)]
pub struct AutolockModuleState {
    enabled: bool,
    socket_filepath: PathBuf,
}

impl AutolockModule {
    pub fn new() -> anyhow::Result<Self> {
        let xdg_dirs = xdg::BaseDirectories::new()?;
        let signals = signal_hook::iterator::Signals::new([signal_hook::consts::signal::SIGUSR1])?;
        Ok(Self { xdg_dirs, signals })
    }

    fn try_update(&mut self) -> anyhow::Result<AutolockModuleState> {
        // Get socket filepath
        let socket_filepath = self
            .xdg_dirs
            .find_runtime_file("xidlehook/autolock.socket")
            .ok_or_else(|| anyhow::anyhow!("Unable to find xidlehook socket"))?;

        // Run xidlehook-client
        let output = Command::new("xidlehook-client")
            .args([
                "--socket",
                socket_filepath
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("Invalid socket filepath"))?,
                "query",
            ])
            .stderr(Stdio::null())
            .output()?;
        output
            .status
            .exit_ok()
            .context("xidlehook-client exited with error")?;

        // Parse output
        let output_str = String::from_utf8_lossy(&output.stdout);
        let enabled = !output_str.contains("disabled: true,\n");

        Ok(AutolockModuleState {
            enabled,
            socket_filepath,
        })
    }
}

impl RenderablePolybarModule for AutolockModule {
    type State = Option<AutolockModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if let Some(prev_state) = prev_state {
            match prev_state {
                // Nominal
                Some(_) => {
                    self.signals.forever().next();
                }
                // Error occured
                None => sleep(Duration::from_secs(1)),
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
            Some(state) => match state.enabled {
                true => markup::action(
                    "󱫗",
                    markup::PolybarAction {
                        type_: markup::PolybarActionType::ClickLeft,
                        command: format!("xidlehook-client --socket {} control --action disable && pkill -USR1 -f '{} autolock$'", state.socket_filepath.to_str().unwrap(), env!("CARGO_PKG_NAME")),
                    },
                ),
                false => markup::action(
                    &markup::style("󱫖", None, Some(theme::Color::Notice), None, None),
                    markup::PolybarAction {
                        type_: markup::PolybarActionType::ClickLeft,
                        command: format!("xidlehook-client --socket {} control --action enable && pkill -USR1 -f '{} autolock$'", state.socket_filepath.to_str().unwrap(), env!("CARGO_PKG_NAME")),
                    },
                ),
            },
            None => markup::style("", Some(theme::Color::Attention), None, None, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render() {
        let module = AutolockModule::new().unwrap();
        let runtime_dir = module.xdg_dirs.find_runtime_file(".").unwrap();

        let state = Some(AutolockModuleState {
            enabled: true,
            socket_filepath: runtime_dir.join("xidlehook/autolock.socket"),
        });
        assert_eq!(
            module.render(&state),
            format!("%{{A1:xidlehook-client --socket {}/xidlehook/autolock.socket control --action disable && pkill -USR1 -f \'polybar-modules autolock$\':}}\u{f1ad7}%{{A}}", runtime_dir.to_str().unwrap())
        );

        let state = Some(AutolockModuleState {
            enabled: false,
            socket_filepath: runtime_dir.join("xidlehook/autolock.socket"),
        });
        assert_eq!(
            module.render(&state),
            format!("%{{A1:xidlehook-client --socket {}/xidlehook/autolock.socket control --action enable && pkill -USR1 -f \'polybar-modules autolock$\':}}%{{u#b58900}}%{{+u}}\u{f1ad6}%{{-u}}%{{A}}", runtime_dir.to_str().unwrap())
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
