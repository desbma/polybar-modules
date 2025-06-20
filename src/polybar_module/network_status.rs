use std::{
    cmp::min,
    collections::HashMap,
    io::{ErrorKind, Read as _},
    os::unix::io::AsRawFd as _,
    process::{Child, Command, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

use sysinfo::Networks;

use crate::{
    config, markup,
    polybar_module::{NetworkMode, PolybarModuleEnv, RenderablePolybarModule},
    theme::{self, ICON_WARNING},
};

const PING_AVG_COUNT: usize = 3;
const AGGREGATE_DELAY: Duration = Duration::from_millis(200);

pub(crate) struct NetworkStatusModule {
    env: PolybarModuleEnv,
    cfg: config::NetworkStatusModuleConfig,
    ping_childs: Vec<Child>,
    poller: mio::Poll,
    poller_events: mio::Events,
    host_state_history: Vec<bounded_vec_deque::BoundedVecDeque<bool>>,
    ping_child_deaths: HashMap<usize, Instant>,
    ping_child_last_reachable: HashMap<usize, Instant>,
    networks: Networks,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct NetworkStatusModuleState {
    reachable_hosts: Vec<bool>,
    vpn: Vec<String>,
}

impl NetworkStatusModule {
    pub(crate) fn new(cfg: config::NetworkStatusModuleConfig) -> anyhow::Result<Self> {
        let env = PolybarModuleEnv::new();
        let mut ping_childs = Vec::with_capacity(cfg.hosts.len());
        let poller = mio::Poll::new()?;
        let poller_registry = poller.registry();
        let now = Instant::now();
        let mut ping_child_last_output = HashMap::new();
        for (i, host) in cfg.hosts.iter().enumerate() {
            // Start ping process & register poller event source
            let child = Self::setup_ping_child(&host.host, i, poller_registry, &env)?;
            ping_childs.push(child);
            ping_child_last_output.insert(i, now);
        }
        let poller_events = mio::Events::with_capacity(ping_childs.len());

        let host_state_history =
            vec![
                bounded_vec_deque::BoundedVecDeque::with_capacity(PING_AVG_COUNT, PING_AVG_COUNT);
                ping_childs.len()
            ];
        let ping_child_deaths = HashMap::new();

        let networks = Networks::new();

        Ok(Self {
            env,
            cfg,
            ping_childs,
            poller,
            poller_events,
            host_state_history,
            ping_child_deaths,
            ping_child_last_reachable: ping_child_last_output,
            networks,
        })
    }

    fn setup_ping_child(
        host: &str,
        idx: usize,
        poller_registry: &mio::Registry,
        env: &PolybarModuleEnv,
    ) -> anyhow::Result<Child> {
        let ping_period_s = Self::get_ping_period(env).as_secs();

        // Start ping process
        let child = Command::new("ping")
            .args([
                "-O",
                "-W",
                &format!("{ping_period_s}"),
                "-i",
                &format!("{ping_period_s}"),
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

    #[expect(clippy::too_many_lines)]
    fn try_update(&mut self) -> anyhow::Result<NetworkStatusModuleState> {
        let now = Instant::now();
        let poller_registry = self.poller.registry();
        let ping_period = Self::get_ping_period(&self.env);
        let mut buffer = vec![0; 65536];

        for event in self.poller_events.iter().filter(|e| e.is_readable()) {
            // Read ping stdout pending data
            let idx = usize::from(event.token());
            buffer.resize(buffer.capacity(), 0);
            let read_count = self
                .ping_childs
                .get_mut(idx)
                .unwrap()
                .stdout
                .as_mut()
                .unwrap()
                .read(&mut buffer)?;
            buffer.truncate(read_count);
            let read_str = String::from_utf8_lossy(&buffer);
            log::trace!(
                "Got output for host {:?}: {:?}",
                self.cfg.hosts.get(idx).unwrap().host,
                read_str
            );

            // Parse ping lines
            for line in read_str.lines() {
                let status = line.ends_with(" ms");
                self.host_state_history
                    .get_mut(idx)
                    .unwrap()
                    .push_back(status);
                if status {
                    self.ping_child_last_reachable.insert(idx, now);
                }
            }
        }

        // Kill processes with no output or failed output for too long
        // This works around a rare bug, if a host becomes unreachable, then reachable again
        // ping sometimes never picks it up again for some reason
        let stale_timeout = Self::get_stale_child_timeout(&self.env);
        for (i, _ts) in self
            .ping_child_last_reachable
            .extract_if(|_i, ts| now.saturating_duration_since(*ts) > stale_timeout)
        {
            log::debug!(
                "ping process for {:?} had no output for a while, killing it",
                self.cfg.hosts.get(i).unwrap().host
            );
            let _ = self.ping_childs.get_mut(i).unwrap().kill(); // ignore error, it can already be dead
        }

        // Build state
        let reachable_hosts = self
            .host_state_history
            .iter()
            .map(|h| h.iter().filter(|e| **e).count() > h.iter().filter(|e| !**e).count())
            .collect();
        self.networks.refresh(true);
        let mut vpn: Vec<String> = self
            .networks
            .list()
            .iter()
            .filter(|i| i.0.starts_with("wg"))
            .map(|i| i.0.to_owned())
            .collect();
        let pgrep_status = Command::new("pgrep")
            .args(["-x", "openvpn"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if pgrep_status.success() {
            vpn.push("ovpn".to_owned());
        }
        vpn.sort();

        // Cleanup newly dead processes
        for (i, child) in &mut self.ping_childs.iter_mut().enumerate() {
            let wait_res = child.try_wait();
            log::trace!(
                "Host {:?} child wait: {:?}",
                self.cfg.hosts.get(i).unwrap().host,
                wait_res
            );
            if let Ok(Some(_)) = wait_res {
                if self.ping_child_deaths.contains_key(&i) {
                    continue;
                }

                log::debug!(
                    "ping process for {:?} has died",
                    self.cfg.hosts.get(i).unwrap().host
                );

                // Keep death timestamp to avoid respawning too fast
                self.ping_child_deaths.insert(i, now);

                // Deregister source
                poller_registry.deregister(&mut mio::unix::SourceFd(
                    &child.stdout.as_ref().unwrap().as_raw_fd(),
                ))?;

                // Add state history entry
                self.host_state_history.get_mut(i).unwrap().push_back(false);
            }
        }

        // Restart new processes if needed
        for (i, _ts) in self
            .ping_child_deaths
            .extract_if(|_i, ts| now.saturating_duration_since(*ts) > ping_period)
        {
            // Setup new child in its place
            *self.ping_childs.get_mut(i).unwrap() = Self::setup_ping_child(
                &self.cfg.hosts.get(i).unwrap().host,
                i,
                poller_registry,
                &self.env,
            )?;
            self.ping_child_last_reachable.insert(i, now);
        }

        Ok(NetworkStatusModuleState {
            reachable_hosts,
            vpn,
        })
    }

    fn get_ping_period(env: &PolybarModuleEnv) -> Duration {
        match env.network_mode() {
            NetworkMode::LowBandwith => Duration::from_secs(5),
            NetworkMode::Unrestricted => Duration::from_secs(1),
        }
    }

    fn get_stale_child_timeout(env: &PolybarModuleEnv) -> Duration {
        min(Self::get_ping_period(env) * 2, Duration::from_secs(5))
    }
}

impl Drop for NetworkStatusModule {
    fn drop(&mut self) {
        for ping_child in &mut self.ping_childs {
            let _ = ping_child.kill();
        }
    }
}

const ICON_NETWORK: &str = "";
const ICON_NETWORK_VPN: &str = "󰒃";

impl RenderablePolybarModule for NetworkStatusModule {
    type State = Option<NetworkStatusModuleState>;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        if prev_state.is_some() {
            // Micro sleep to aggregate several ping events
            sleep(AGGREGATE_DELAY);

            let duration = Self::get_ping_period(&self.env).saturating_sub(AGGREGATE_DELAY);
            log::trace!("Waiting for network events");
            let poll_res = self.poller.poll(&mut self.poller_events, Some(duration));
            if let Err(e) = &poll_res
                && e.kind() == ErrorKind::Interrupted
            {
                // Ignore error, and do not retry, can occur on return from hibernation
                return;
            }
            poll_res.unwrap();
            log::trace!("Poll returned with events {:?}", self.poller_events);
        }
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
                let mut fragments: Vec<String> = vec![markup::style(
                    ICON_NETWORK,
                    Some(theme::Color::MainIcon),
                    None,
                    None,
                    None,
                )];
                for (reachable, host_info) in state.reachable_hosts.iter().zip(&self.cfg.hosts) {
                    fragments.push(markup::style(
                        &host_info.name,
                        (!reachable && host_info.warn_unreachable)
                            .then_some(theme::Color::Attention),
                        (*reachable).then_some(theme::Color::Foreground),
                        None,
                        None,
                    ));
                }
                if !state.vpn.is_empty() {
                    fragments.push(format!(
                        " {}",
                        markup::style(
                            ICON_NETWORK_VPN,
                            Some(theme::Color::MainIcon),
                            None,
                            None,
                            None,
                        )
                    ));
                    for wireguard_interface in &state.vpn {
                        fragments.push(markup::style(
                            wireguard_interface,
                            None,
                            Some(theme::Color::Foreground),
                            None,
                            None,
                        ));
                    }
                }
                fragments.join(" ")
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
        let module = NetworkStatusModule::new(config::NetworkStatusModuleConfig {
            hosts: vec![
                config::NetworkStatusHost {
                    name: "h1".to_owned(),
                    host: "h1.example.com".to_owned(),
                    warn_unreachable: false,
                },
                config::NetworkStatusHost {
                    name: "h2".to_owned(),
                    host: "h2.example.com".to_owned(),
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
            vpn: vec!["i1".to_owned()],
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} %{u#93a1a1}%{+u}h1%{-u} %{F#cb4b16}h2%{F-}  %{F#eee8d5}󰒃%{F-} %{u#93a1a1}%{+u}i1%{-u}"
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
