use std::{
    collections::BTreeMap,
    fs,
    io::{ErrorKind, Read as _},
    os::unix::{
        io::AsRawFd as _,
        net::{UnixListener, UnixStream},
    },
    time::Duration,
};

use crate::{
    markup,
    polybar_module::RenderablePolybarModule,
    theme::{self, ICON_WARNING},
};

pub(crate) struct ProgressBarServerModule {
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

impl ProgressBarServerModule {
    pub(crate) fn new() -> anyhow::Result<Self> {
        let binary_name = env!("CARGO_PKG_NAME");
        let xdg_dirs = xdg::BaseDirectories::with_prefix(binary_name);
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
                    log::warn!("Unhandled event: {event:?}");
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
                        let progress = u32::from(*buffer.get(read_count - 1).unwrap());
                        if progress <= 100 {
                            self.cur_progress.insert(token, progress);
                        } else {
                            log::warn!("Received invalid progress {progress:?}");
                        }
                    } else {
                        client_disconnected = true;
                    }
                } else if event.is_read_closed() {
                    // Client disconnected
                    client_disconnected = true;
                } else {
                    log::warn!("Unhandled event: {event:?}");
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

    fn render_progress(progress: u32) -> String {
        assert!(progress <= 100);
        let icon = if progress == 0 {
            PROGRESS_ICONS[0]
        } else {
            #[expect(clippy::indexing_slicing)]
            PROGRESS_ICONS[1 + (progress as usize - 1) * (PROGRESS_ICONS.len() - 2) / 99]
        };
        markup::Markup::new(icon)
            .fg(theme::Color::Foreground)
            .into_string()
    }
}

const PROGRESS_ICONS: [&str; 9] = [
    "", // nf-fa-hourglass_start
    "󰪞", // nf-md-circle_slice_1
    "󰪟", // nf-md-circle_slice_2
    "󰪠", // nf-md-circle_slice_3
    "󰪡", // nf-md-circle_slice_4
    "󰪢", // nf-md-circle_slice_5
    "󰪣", // nf-md-circle_slice_6
    "󰪤", // nf-md-circle_slice_7
    "󰪥", // nf-md-circle_slice_8
];

const ICON_PROGRESSBAR_SERVER: &str = "";

impl RenderablePolybarModule for ProgressBarServerModule {
    type State = Option<ProgressBarServerModuleState>;

    fn wait_update(&mut self, _prev_state: Option<&Self::State>) {
        loop {
            let poll_res = self.poller.poll(&mut self.poller_events, None);
            if let Err(e) = &poll_res
                && e.kind() == ErrorKind::Interrupted
            {
                continue;
            }
            poll_res.unwrap();
            break;
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
                if state.progress.is_empty() {
                    String::new()
                } else {
                    let progress_chars: String = state
                        .progress
                        .iter()
                        .map(|p| Self::render_progress(*p))
                        .collect();
                    format!(
                        "{} {}",
                        markup::Markup::new(ICON_PROGRESSBAR_SERVER)
                            .fg(theme::Color::MainIcon)
                            .into_string(),
                        progress_chars,
                    )
                }
            }
            None => markup::Markup::new(ICON_WARNING)
                .fg(theme::Color::Attention)
                .into_string(),
        }
    }
}

#[cfg(test)]
#[expect(clippy::shadow_unrelated)]
mod tests {
    use super::*;

    #[test]
    fn test_render() {
        let module = ProgressBarServerModule::new().unwrap();

        let state = Some(ProgressBarServerModuleState { progress: vec![] });
        assert_eq!(module.render(&state), "");

        let state = Some(ProgressBarServerModuleState { progress: vec![0] });
        assert_eq!(module.render(&state), "%{F#f1e9d2}%{F-} %{F#8faaab}%{F-}");

        let state = Some(ProgressBarServerModuleState { progress: vec![1] });
        assert_eq!(module.render(&state), "%{F#f1e9d2}%{F-} %{F#8faaab}󰪞%{F-}");

        let state = Some(ProgressBarServerModuleState { progress: vec![50] });
        assert_eq!(module.render(&state), "%{F#f1e9d2}%{F-} %{F#8faaab}󰪡%{F-}");

        let state = Some(ProgressBarServerModuleState {
            progress: vec![100],
        });
        assert_eq!(module.render(&state), "%{F#f1e9d2}%{F-} %{F#8faaab}󰪥%{F-}");

        let state = Some(ProgressBarServerModuleState {
            progress: vec![30, 100],
        });
        assert_eq!(
            module.render(&state),
            "%{F#f1e9d2}%{F-} %{F#8faaab}󰪠%{F-}%{F#8faaab}󰪥%{F-}"
        );

        let state = Some(ProgressBarServerModuleState {
            progress: vec![30, 50, 100],
        });
        assert_eq!(
            module.render(&state),
            "%{F#f1e9d2}%{F-} %{F#8faaab}󰪠%{F-}%{F#8faaab}󰪡%{F-}%{F#8faaab}󰪥%{F-}"
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#d56500}%{F-}");
    }
}
