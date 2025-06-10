use std::{
    io::{BufRead as _, BufReader, ErrorKind},
    os::fd::AsRawFd as _,
    process::{Child, Command, Stdio},
};

use crate::{markup, polybar_module::RenderablePolybarModule, theme};

pub(crate) struct PlayerModule {
    playerctl: Child,
    poller: mio::Poll,
    max_len: usize,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct PlayerModuleState {
    player: String,
    status: String,
    artist: String,
    album: String,
    title: String,
}

impl PlayerModule {
    pub(crate) fn new(max_len: usize) -> anyhow::Result<Self> {
        let playerctl = Command::new("playerctl")
            .args([
                "metadata",
                "--follow",
                "--format",
                "{{playerName}}│{{status}}│{{ artist }}│{{album}}│{{ title }}",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let poller = mio::Poll::new()?;

        let stdout = playerctl.stdout.as_ref().unwrap();
        poller.registry().register(
            &mut mio::unix::SourceFd(&stdout.as_raw_fd()),
            mio::Token(0),
            mio::Interest::READABLE,
        )?;

        Ok(Self {
            playerctl,
            poller,
            max_len,
        })
    }
}

impl Drop for PlayerModule {
    fn drop(&mut self) {
        let _ = self.playerctl.kill();
    }
}

const ICON_PLAYER: &str = "";
const ICON_PLAYER_PLAYING: &str = "";
const ICON_PLAYER_PAUSED: &str = "";
const ICON_PLAYER_STOPPED: &str = "";

impl RenderablePolybarModule for PlayerModule {
    type State = Option<PlayerModuleState>;

    fn wait_update(&mut self, _prev_state: Option<&Self::State>) {
        let mut poller_events = mio::Events::with_capacity(1);
        log::trace!("Waiting for stdout data");
        loop {
            let poll_res = self.poller.poll(&mut poller_events, None);
            if let Err(e) = &poll_res
                && e.kind() == ErrorKind::Interrupted
            {
                // Ignore error, can occur on return from hibernation
                continue;
            }
            poll_res.unwrap();
            log::trace!("Poll returned with events {poller_events:?}");
            if poller_events.iter().any(mio::event::Event::is_readable) {
                break;
            }
        }
    }

    fn update(&mut self) -> Self::State {
        let stdout = self.playerctl.stdout.as_mut().unwrap();
        let output = BufReader::new(stdout).lines().next().unwrap().unwrap();
        if output.is_empty() {
            None
        } else {
            let mut tokens = output.split('│');
            Some(PlayerModuleState {
                player: tokens.next().unwrap().to_owned(),
                status: tokens.next().unwrap().to_owned(),
                artist: tokens.next().unwrap().to_owned(),
                album: tokens.next().unwrap().to_owned(),
                title: tokens.next().unwrap().to_owned(),
            })
        }
    }

    fn render(&self, state: &Self::State) -> String {
        match state {
            Some(state) => {
                let status = match state.status.as_str() {
                    "Playing" => ICON_PLAYER_PLAYING,
                    "Paused" => ICON_PLAYER_PAUSED,
                    "Stopped" => ICON_PLAYER_STOPPED,
                    _ => state.status.as_str(),
                };
                let player = match state.player.as_str() {
                    "mpv" => "",
                    _ => state.player.as_str(),
                };
                let mut s = String::new();
                let base_tokens_candidates = [
                    (
                        vec![
                            player,
                            status,
                            state.artist.as_str(),
                            state.album.as_str(),
                            state.title.as_str(),
                        ],
                        2,
                    ),
                    (
                        vec![
                            status,
                            state.artist.as_str(),
                            state.album.as_str(),
                            state.title.as_str(),
                        ],
                        1,
                    ),
                    (vec![status, state.artist.as_str(), state.title.as_str()], 1),
                    (vec![status, state.title.as_str()], 1),
                ];
                for (base_tokens, sep_idx) in base_tokens_candidates {
                    let tokens: Vec<_> =
                        base_tokens.into_iter().filter(|t| !t.is_empty()).collect();
                    let (first_tokens, other_tokens) = tokens.split_at(sep_idx);
                    s = format!(
                        "{} {} {}",
                        markup::style(ICON_PLAYER, Some(theme::Color::MainIcon), None, None, None),
                        first_tokens.join(" "),
                        other_tokens.join(&markup::style(
                            " / ",
                            Some(theme::Color::Unfocused),
                            None,
                            None,
                            None
                        ))
                    );
                    if s.len() <= self.max_len {
                        return s;
                    }
                }
                theme::ellipsis(&s, Some(self.max_len))
            }
            None => String::new(),
        }
    }
}
