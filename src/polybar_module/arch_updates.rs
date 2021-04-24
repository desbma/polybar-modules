use crate::markup;
use crate::polybar_module::{PolybarModuleEnv, RenderablePolybarModule, RuntimeMode};
use crate::theme;

pub struct ArchUpdatesModule {
    env: PolybarModuleEnv,
}

#[derive(Debug, PartialEq)]
pub struct ArchUpdatesModuleState {
    repo_update_count: u32,
    repo_security_update_count: u32,
    aur_update_count: u32,
}

impl ArchUpdatesModule {
    pub fn new() -> ArchUpdatesModule {
        let env = PolybarModuleEnv::new();
        ArchUpdatesModule { env }
    }

    fn try_update(&mut self) -> anyhow::Result<ArchUpdatesModuleState> {
        Ok(ArchUpdatesModuleState {
            repo_update_count: 0,
            repo_security_update_count: 0,
            aur_update_count: 0,
        })
    }
}

impl RenderablePolybarModule for ArchUpdatesModule {
    type State = Option<ArchUpdatesModuleState>;

    fn wait_update(&mut self, first_update: bool) {
        if !first_update {
            std::thread::sleep(std::time::Duration::from_secs(60 * 10));
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
                        r += &format!("+{}", state.aur_update_count);
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
