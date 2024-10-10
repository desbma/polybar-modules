use std::{
    collections::BTreeMap,
    fs,
    io::{ErrorKind, Read},
    os::unix::{
        io::AsRawFd,
        net::{UnixListener, UnixStream},
    },
    time::Duration,
};

use crate::{markup, polybar_module::RenderablePolybarModule, theme};

pub(crate) struct ProgressBarServerModule {
    max_len: usize,
    listener: UnixListener,
    clients: BTreeMap<usize, UnixStream>,
    next_client_id: usize,
    poller: mio::Poll,
    poller_events: mio::Events,
    cur_progress: BTreeMap<usize, u32>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ProgressBarServerModuleState {
    progress: Vec<u32>,
}

const RAMP_ICONS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

impl ProgressBarServerModule {
    pub(crate) fn new(max_len: usize) -> anyhow::Result<Self> {
        let binary_name = env!("CARGO_PKG_NAME");
        let xdg_dirs = xdg::BaseDirectories::with_prefix(binary_name)?;
        let socket_filepath = match xdg_dirs.find_runtime_file("progressbar_server.socket") {
            Some(socket_filepath) => {
                fs::remove_file(&socket_filepath)?;
                socket_filepath
            }
            None => xdg_dirs.place_runtime_file("progressbar_server.socket")?,
        };
        let listener = UnixListener::bind(socket_filepath)?;
        let poller = mio::Poll::new()?;
        let poller_registry = poller.registry();
        poller_registry.register(
            &mut mio::unix::SourceFd(&listener.as_raw_fd()),
            mio::Token(0),
            mio::Interest::READABLE,
        )?;
        Ok(Self {
            max_len,
            listener,
            clients: BTreeMap::new(),
            next_client_id: 1,
            poller,
            poller_events: mio::Events::with_capacity(4),
            cur_progress: BTreeMap::new(),
        })
    }

    fn try_update(&mut self) -> anyhow::Result<ProgressBarServerModuleState> {
        let poller_registry = self.poller.registry();
        for event in &self.poller_events {
            let token = usize::from(event.token());
            if token == 0 {
                // Server socket event
                if event.is_readable() {
                    // New client
                    log::debug!("New client");
                    let client_stream = self.listener.incoming().next().unwrap()?;
                    client_stream.set_read_timeout(Some(Duration::from_millis(1)))?;
                    let client_id = self.next_client_id;
                    self.next_client_id += 1;
                    poller_registry.register(
                        &mut mio::unix::SourceFd(&client_stream.as_raw_fd()),
                        mio::Token(client_id),
                        mio::Interest::READABLE,
                    )?;
                    self.clients.insert(client_id, client_stream);
                } else {
                    log::warn!("Unhandled event: {:?}", event);
                }
            } else {
                let mut client_disconnected = false;

                // Client socket event
                if event.is_readable() {
                    // Progress update
                    let mut client_stream = self.clients.get(&token).unwrap();
                    let mut buffer = [0; 4096];
                    let read_count = client_stream.read(&mut buffer)?;
                    if read_count > 0 {
                        let progress = u32::from(buffer[read_count - 1]);
                        if progress <= 100 {
                            self.cur_progress.insert(token, progress);
                        } else {
                            log::warn!("Received invalid progress {:?}", progress);
                        }
                    } else {
                        client_disconnected = true;
                    }
                } else if event.is_read_closed() {
                    // Client disconnected
                    client_disconnected = true;
                } else {
                    log::warn!("Unhandled event: {:?}", event);
                }

                if client_disconnected {
                    log::debug!("Client disconnected");
                    let client_stream = self.clients.get(&token).unwrap();
                    poller_registry
                        .deregister(&mut mio::unix::SourceFd(&client_stream.as_raw_fd()))?;
                    self.clients.remove(&token);
                    self.cur_progress.remove(&token);
                }
            }
        }

        Ok(ProgressBarServerModuleState {
            progress: self.cur_progress.values().copied().collect(),
        })
    }

    fn render_progress(progress: u32, len: usize) -> String {
        assert!(len >= 1);
        assert!(progress <= 100);
        if len == 1 {
            RAMP_ICONS[progress as usize / (100 / (RAMP_ICONS.len() - 1))].to_owned()
        } else {
            let progress_chars = len * progress as usize / 100;
            let remaining_chars = len - progress_chars;
            format!(
                "{}{}",
                "■".repeat(progress_chars),
                " ".repeat(remaining_chars)
            )
        }
    }
}

impl RenderablePolybarModule for ProgressBarServerModule {
    type State = Option<ProgressBarServerModuleState>;

    fn wait_update(&mut self, _prev_state: Option<&Self::State>) {
        loop {
            let poll_res = self.poller.poll(&mut self.poller_events, None);
            if let Err(e) = &poll_res {
                if e.kind() == ErrorKind::Interrupted {
                    continue;
                }
            }
            poll_res.unwrap();
            break;
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
            #[allow(clippy::cast_possible_truncation)]
            Some(state) => {
                if state.progress.is_empty() {
                    String::new()
                } else if state.progress.len() == 1 {
                    format!(
                        "{} {} {}",
                        markup::style("", Some(theme::Color::Foreground), None, None, None),
                        state.progress.len(),
                        Self::render_progress(state.progress[0], self.max_len - 2)
                    )
                } else if state.progress.len() == 2 {
                    format!(
                        "{} {} {} {}",
                        markup::style("", Some(theme::Color::Foreground), None, None, None),
                        state.progress.len(),
                        Self::render_progress(state.progress[0], (self.max_len - 3) / 2),
                        Self::render_progress(state.progress[1], (self.max_len - 3) / 2),
                    )
                } else {
                    // Average progress, then maximum
                    format!(
                        "{} {} {} {}",
                        markup::style("", Some(theme::Color::Foreground), None, None, None),
                        state.progress.len(),
                        Self::render_progress(
                            state.progress.iter().sum::<u32>() / state.progress.len() as u32,
                            (self.max_len - 3) / 2
                        ),
                        Self::render_progress(
                            *state.progress.iter().max().unwrap(),
                            (self.max_len - 3) / 2
                        ),
                    )
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
        let module = ProgressBarServerModule::new(20).unwrap();

        let state = Some(ProgressBarServerModuleState { progress: vec![] });
        assert_eq!(module.render(&state), "");

        let state = Some(ProgressBarServerModuleState { progress: vec![30] });
        assert_eq!(
            module.render(&state),
            "%{F#93a1a1}%{F-} 1 ■■■■■             "
        );

        let state = Some(ProgressBarServerModuleState {
            progress: vec![30, 40],
        });
        assert_eq!(
            module.render(&state),
            "%{F#93a1a1}%{F-} 2 ■■       ■■■     "
        );

        let state = Some(ProgressBarServerModuleState {
            progress: vec![30, 40, 50],
        });
        assert_eq!(
            module.render(&state),
            "%{F#93a1a1}%{F-} 3 ■■■      ■■■■    "
        );

        let module = ProgressBarServerModule::new(5).unwrap();

        let state = Some(ProgressBarServerModuleState { progress: vec![30] });
        assert_eq!(module.render(&state), "%{F#93a1a1}%{F-} 1    ");

        let state = Some(ProgressBarServerModuleState {
            progress: vec![100],
        });
        assert_eq!(module.render(&state), "%{F#93a1a1}%{F-} 1 ■■■");

        let state = Some(ProgressBarServerModuleState {
            progress: vec![30, 45],
        });
        assert_eq!(module.render(&state), "%{F#93a1a1}%{F-} 2 ▃ ▄");

        let state = Some(ProgressBarServerModuleState {
            progress: vec![30, 100],
        });
        assert_eq!(module.render(&state), "%{F#93a1a1}%{F-} 2 ▃ █");

        let state = Some(ProgressBarServerModuleState {
            progress: vec![30, 40, 50],
        });
        assert_eq!(module.render(&state), "%{F#93a1a1}%{F-} 3 ▃ ▄");

        let state = Some(ProgressBarServerModuleState {
            progress: vec![30, 100, 50],
        });
        assert_eq!(module.render(&state), "%{F#93a1a1}%{F-} 3 ▅ █");

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
