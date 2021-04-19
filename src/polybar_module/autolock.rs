use crate::markup;
use crate::polybar_module::RenderablePolybarModule;
use crate::theme;

pub struct AutolockModule {
    xdg_dirs: xdg::BaseDirectories,
    signals: signal_hook::iterator::Signals,
}

#[derive(Debug, PartialEq)]
pub struct AutolockModuleState {
    enabled: bool,
    socket_filepath: String,
}

impl AutolockModule {
    pub fn new() -> AutolockModule {
        let xdg_dirs = xdg::BaseDirectories::new().unwrap();
        let signals =
            signal_hook::iterator::Signals::new(&[signal_hook::consts::signal::SIGUSR1]).unwrap();
        AutolockModule { xdg_dirs, signals }
    }

    fn try_update(&mut self) -> anyhow::Result<AutolockModuleState> {
        // Get socket filepath
        let socket_filepath = self
            .xdg_dirs
            .find_runtime_file("xidlehook/autolock.socket")
            .ok_or_else(|| anyhow::anyhow!("Unable to find xidlehook socket"))?
            .as_os_str()
            .to_os_string()
            .into_string()
            .unwrap();

        // Run xidlehook-client
        let output = std::process::Command::new("xidlehook-client")
            .args(&["--socket", &socket_filepath, "query"])
            .stderr(std::process::Stdio::null())
            .output()?;
        if !output.status.success() {
            anyhow::bail!("xidlehook-client invocation failed");
        }

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

    fn wait_update(&mut self, first_update: bool) {
        if !first_update {
            self.signals.forever().next();
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
                    "",
                    markup::PolybarAction {
                        type_: markup::PolybarActionType::ClickLeft,
                        command: format!("xidlehook-client --socket {} control --action disable && pkill -USR1 -f '{} autolock$'", state.socket_filepath, env!("CARGO_PKG_NAME")),
                    },
                ),
                false => markup::action(
                    &markup::style("", None, Some(theme::Color::Notice), None, None),
                    markup::PolybarAction {
                        type_: markup::PolybarActionType::ClickLeft,
                        command: format!("xidlehook-client --socket {} control --action enable && pkill -USR1 -f '{} autolock$'", state.socket_filepath, env!("CARGO_PKG_NAME")),
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
        let module = AutolockModule::new();
        // For some reason get_runtime_directory is not public
        let runtime_dir = module
            .xdg_dirs
            .find_runtime_file(".")
            .unwrap()
            .as_os_str()
            .to_os_string()
            .into_string()
            .unwrap();

        let state = Some(AutolockModuleState {
            enabled: true,
            socket_filepath: format!("{}/xidlehook/autolock.socket", runtime_dir),
        });
        assert_eq!(
            module.render(&state),
            format!("%{{A1:xidlehook-client --socket {}/xidlehook/autolock.socket control --action disable && pkill -USR1 -f \'polybar-modules autolock$\':}}%{{A}}", runtime_dir)
        );
        let state = Some(AutolockModuleState {
            enabled: false,
            socket_filepath: format!("{}/xidlehook/autolock.socket", runtime_dir),
        });
        assert_eq!(
            module.render(&state),
            format!("%{{A1:xidlehook-client --socket {}/xidlehook/autolock.socket control --action enable && pkill -USR1 -f \'polybar-modules autolock$\':}}%{{u#b58900}}%{{+u}}%{{-u}}%{{A}}", runtime_dir)
        );
        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
