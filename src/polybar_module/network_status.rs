use std::thread::sleep;
use std::time::Duration;

use crate::config;
use crate::markup;
use crate::polybar_module::RenderablePolybarModule;
use crate::theme;

pub struct NetworkStatusModule {
    cfg: config::NetworkStatusModuleConfig,
}

#[derive(Debug, PartialEq)]
pub struct NetworkStatusModuleState {
    reachable: Vec<bool>,
}

impl NetworkStatusModule {
    pub fn new(cfg: config::NetworkStatusModuleConfig) -> anyhow::Result<NetworkStatusModule> {
        Ok(NetworkStatusModule { cfg })
    }

    fn try_update(&mut self) -> anyhow::Result<NetworkStatusModuleState> {
        Ok(NetworkStatusModuleState {
            reachable: vec![false; self.cfg.hosts.len()],
        })
    }
}

impl RenderablePolybarModule for NetworkStatusModule {
    type State = Option<NetworkStatusModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if prev_state.is_some() {
            sleep(Duration::from_secs(999)); // TODO remove this
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
                let mut fragments: Vec<String> = vec![markup::style(
                    "",
                    Some(theme::Color::MainIcon),
                    None,
                    None,
                    None,
                )];
                for (reachable, host_info) in state.reachable.iter().zip(&self.cfg.hosts) {
                    fragments.push(markup::style(
                        &host_info.name,
                        if !reachable && host_info.warn_unreachable {
                            Some(theme::Color::Attention)
                        } else {
                            None
                        },
                        if *reachable {
                            Some(theme::Color::Foreground)
                        } else {
                            None
                        },
                        None,
                        None,
                    ));
                }
                fragments.join(" ")
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
        let module = NetworkStatusModule::new(config::NetworkStatusModuleConfig {
            hosts: vec![
                config::NetworkStatusHost {
                    name: "h1".to_string(),
                    host: "h1.example.com".to_string(),
                    warn_unreachable: false,
                },
                config::NetworkStatusHost {
                    name: "h2".to_string(),
                    host: "h2.example.com".to_string(),
                    warn_unreachable: true,
                },
            ],
        })
        .unwrap();

        let state = Some(NetworkStatusModuleState {
            reachable: vec![true, true],
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} %{u#93a1a1}%{+u}h1%{-u} %{u#93a1a1}%{+u}h2%{-u}"
        );

        let state = Some(NetworkStatusModuleState {
            reachable: vec![false, true],
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} h1 %{u#93a1a1}%{+u}h2%{-u}"
        );

        let state = Some(NetworkStatusModuleState {
            reachable: vec![true, false],
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} %{u#93a1a1}%{+u}h1%{-u} %{F#cb4b16}h2%{F-}"
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
