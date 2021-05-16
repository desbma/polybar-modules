use std::collections::HashMap;
use std::io::{ErrorKind, Read};
use std::os::unix::io::AsRawFd;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use sysinfo::{NetworksExt, System, SystemExt};

use crate::config;
use crate::markup;
use crate::polybar_module::{PolybarModuleEnv, RenderablePolybarModule, RuntimeMode};
use crate::theme;

const PING_AVG_COUNT: usize = 3;

pub struct NetworkStatusModule {
    env: PolybarModuleEnv,
    cfg: config::NetworkStatusModuleConfig,
    ping_childs: Vec<Child>,
    poller: mio::Poll,
    poller_events: mio::Events,
    host_state_history: Vec<bounded_vec_deque::BoundedVecDeque<bool>>,
    ping_child_deaths: HashMap<usize, Instant>,
    system: Box<System>,
}

#[derive(Debug, PartialEq)]
pub struct NetworkStatusModuleState {
    reachable_hosts: Vec<bool>,
    vpn: Vec<String>,
}

impl NetworkStatusModule {
    pub fn new(cfg: config::NetworkStatusModuleConfig) -> anyhow::Result<NetworkStatusModule> {
        let env = PolybarModuleEnv::new();
        let mut ping_childs = Vec::with_capacity(cfg.hosts.len());
        let poller = mio::Poll::new().unwrap();
        let poller_registry = poller.registry();
        for (i, host) in cfg.hosts.iter().enumerate() {
            // Start ping process & register poller event source
            let child = Self::setup_ping_child(&host.host, i, &poller_registry, &env)?;

            ping_childs.push(child);
        }
        let poller_events = mio::Events::with_capacity(ping_childs.len());

        let host_state_history =
            vec![
                bounded_vec_deque::BoundedVecDeque::with_capacity(PING_AVG_COUNT, PING_AVG_COUNT);
                ping_childs.len()
            ];
        let ping_child_deaths = HashMap::new();

        let system = Box::new(SystemExt::new_with_specifics(
            sysinfo::RefreshKind::new().with_networks_list(),
        ));

        Ok(NetworkStatusModule {
            env,
            cfg,
            ping_childs,
            poller,
            poller_events,
            host_state_history,
            ping_child_deaths,
            system,
        })
    }

    fn setup_ping_child(
        host: &str,
        idx: usize,
        poller_registry: &mio::Registry,
        env: &PolybarModuleEnv,
    ) -> anyhow::Result<Child> {
        let ping_period_s = Self::get_ping_period(&env).as_secs();

        // Start ping process
        let child = Command::new("ping")
            .args(&[
                "-O",
                "-W",
                &format!("{}", ping_period_s),
                "-i",
                &format!("{}", ping_period_s),
                "-n",
                host,
            ])
            .env("LANG", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        // Setup poll event source
        poller_registry.register(
            &mut mio::unix::SourceFd(&child.stdout.as_ref().unwrap().as_raw_fd()),
            mio::Token(idx),
            mio::Interest::READABLE,
        )?;

        Ok(child)
    }

    fn try_update(&mut self) -> anyhow::Result<NetworkStatusModuleState> {
        let now = Instant::now();
        let poller_registry = self.poller.registry();

        let mut buffer = [0; 65536];

        for event in self.poller_events.iter().filter(|e| e.is_readable()) {
            // Read ping stdout pending data
            let idx = usize::from(event.token());
            let read_count = self.ping_childs[idx]
                .stdout
                .as_mut()
                .unwrap()
                .read(&mut buffer)?;
            let read_str = String::from_utf8_lossy(&buffer[0..read_count]);
            log::debug!("Got output: {:?}", read_str);

            // Parse ping lines
            for line in read_str.lines() {
                self.host_state_history[idx].push_back(line.ends_with(" ms"));
            }
        }

        // Build state
        let reachable_hosts = self
            .host_state_history
            .iter()
            .map(|h| h.iter().filter(|e| **e).count() > h.iter().filter(|e| !**e).count())
            .collect();
        self.system.refresh_networks_list();
        let mut vpn: Vec<String> = self
            .system
            .get_networks()
            .iter()
            .filter(|i| i.0.starts_with("wg"))
            .map(|i| i.0.to_owned())
            .collect();
        let pgrep_status = Command::new("pgrep")
            .args(&["-x", "openvpn"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if pgrep_status.success() {
            vpn.push("ovpn".to_string());
        }
        vpn.sort();

        // Cleanup newly dead processes
        for (i, child) in &mut self.ping_childs.iter_mut().enumerate() {
            if let Ok(Some(_)) = child.try_wait() {
                if self.ping_child_deaths.contains_key(&i) {
                    continue;
                }

                log::debug!("ping process for {:?} has died", self.cfg.hosts[i].host);

                // Keep death timestamp to avoid respawning too fast
                self.ping_child_deaths.insert(i, Instant::now());

                // Deregister source
                poller_registry.deregister(&mut mio::unix::SourceFd(
                    &child.stdout.as_ref().unwrap().as_raw_fd(),
                ))?;

                // Add state history entry
                self.host_state_history[i].push_back(false);
            }
        }

        // Restart new processes if needed
        let ping_period = Self::get_ping_period(&self.env);
        for (i, _ts) in self
            .ping_child_deaths
            .drain_filter(|_i, ts| now >= *ts + ping_period)
        {
            // Setup new child in its place
            self.ping_childs[i] =
                Self::setup_ping_child(&self.cfg.hosts[i].host, i, &poller_registry, &self.env)?;
        }

        Ok(NetworkStatusModuleState {
            reachable_hosts,
            vpn,
        })
    }

    fn get_ping_period(env: &PolybarModuleEnv) -> Duration {
        match env.get_runtime_mode() {
            RuntimeMode::LowNetworkBandwith => Duration::from_secs(5),
            RuntimeMode::Unrestricted => Duration::from_secs(1),
        }
    }
}

impl Drop for NetworkStatusModule {
    #[allow(unused_must_use)]
    fn drop(&mut self) {
        for ping_child in &mut self.ping_childs {
            ping_child.kill();
        }
    }
}

impl RenderablePolybarModule for NetworkStatusModule {
    type State = Option<NetworkStatusModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if prev_state.is_some() {
            // Micro sleep to aggregate several ping events
            sleep(Duration::from_millis(10));

            let duration = Self::get_ping_period(&self.env);
            log::trace!("Waiting for network events");
            let poll_res = self.poller.poll(&mut self.poller_events, Some(duration));
            if let Err(ref e) = poll_res {
                if e.kind() == ErrorKind::Interrupted {
                    // Ignore error, and do not retry, can occur on return from hibernation
                    return;
                }
            }
            poll_res.unwrap();
            log::trace!("Poll returned with events {:?}", self.poller_events);
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
                for (reachable, host_info) in state.reachable_hosts.iter().zip(&self.cfg.hosts) {
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
                if !state.vpn.is_empty() {
                    fragments.push(format!(
                        " {}",
                        markup::style("", Some(theme::Color::MainIcon), None, None, None,)
                    ));
                    for wireguard_interface in &state.vpn {
                        fragments.push(markup::style(
                            &wireguard_interface,
                            None,
                            Some(theme::Color::Foreground),
                            None,
                            None,
                        ));
                    }
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
            reachable_hosts: vec![true, true],
            vpn: vec![],
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} %{u#93a1a1}%{+u}h1%{-u} %{u#93a1a1}%{+u}h2%{-u}"
        );

        let state = Some(NetworkStatusModuleState {
            reachable_hosts: vec![false, true],
            vpn: vec![],
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} h1 %{u#93a1a1}%{+u}h2%{-u}"
        );

        let state = Some(NetworkStatusModuleState {
            reachable_hosts: vec![true, false],
            vpn: vec![],
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} %{u#93a1a1}%{+u}h1%{-u} %{F#cb4b16}h2%{F-}"
        );

        let state = Some(NetworkStatusModuleState {
            reachable_hosts: vec![true, false],
            vpn: vec!["i1".to_string()],
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} %{u#93a1a1}%{+u}h1%{-u} %{F#cb4b16}h2%{F-}  %{F#eee8d5}%{F-} %{u#93a1a1}%{+u}i1%{-u}"
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
