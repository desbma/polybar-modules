use std::{
    fs::File,
    io::{ErrorKind, Read as _},
    mem,
    os::unix::io::AsRawFd as _,
    thread::sleep,
    time::Duration,
};

use crate::{
    markup,
    polybar_module::RenderablePolybarModule,
    theme::{self, ICON_WARNING},
};

pub(crate) struct XmonadModule {
    xdg_dirs: xdg::BaseDirectories,
    pipe: Option<File>,
    poller: mio::Poll,
    pending_data: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct XmonadModuleState {
    layout: String,
}

impl XmonadModule {
    pub(crate) fn new() -> anyhow::Result<Self> {
        let xdg_dirs = xdg::BaseDirectories::new();
        Ok(Self {
            xdg_dirs,
            pipe: None,
            poller: mio::Poll::new()?,
            pending_data: String::new(),
        })
    }

    fn open_pipe(&mut self) -> anyhow::Result<()> {
        self.pipe = None;

        let path = self
            .xdg_dirs
            .find_runtime_file("xmonad/status.pipe")
            .ok_or_else(|| anyhow::anyhow!("No status pipe"))?;
        let pipe = File::open(path)?;

        self.poller = mio::Poll::new()?;
        self.poller.registry().register(
            &mut mio::unix::SourceFd(&pipe.as_raw_fd()),
            mio::Token(0),
            mio::Interest::READABLE,
        )?;

        self.pipe = Some(pipe);
        Ok(())
    }
}

impl RenderablePolybarModule for XmonadModule {
    type State = Option<XmonadModuleState>;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        let prev_state_err = prev_state.as_ref().is_some_and(|o| o.is_none());
        if self.pipe.is_none() || prev_state_err {
            if prev_state_err {
                sleep(Duration::from_secs(1));
            }
            if let Err(e) = self.open_pipe() {
                log::debug!("{e:?}");
                return;
            }
        }

        let mut poller_events = mio::Events::with_capacity(1);
        log::trace!("Waiting for pipe data");
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
                self.pipe
                    .as_ref()
                    .unwrap()
                    .read_to_string(&mut self.pending_data)
                    .unwrap();
                if !self.pending_data.is_empty() {
                    break;
                }
            }
        }
    }

    fn update(&mut self) -> Self::State {
        if self.pipe.is_none() {
            None
        } else {
            assert!(!self.pending_data.is_empty());
            Some(XmonadModuleState {
                layout: mem::take(&mut self.pending_data),
            })
        }
    }

    fn render(&self, state: &Self::State) -> String {
        if let Some(state) = state {
            state
                .layout
                .split(' ')
                .map(|t| {
                    let mut s = t.to_owned();
                    s.truncate(4);
                    s
                })
                .collect::<Vec<String>>()
                .join(" ")
        } else {
            markup::style(
                ICON_WARNING,
                Some(theme::Color::Attention),
                None,
                None,
                None,
            )
        }
    }
}

#[cfg(test)]
#[expect(clippy::shadow_unrelated)]
mod tests {
    use super::*;

    #[test]
    fn test_render() {
        let module = XmonadModule::new().unwrap();

        let state = Some(XmonadModuleState {
            layout: "Spacing Tall".to_owned(),
        });
        assert_eq!(module.render(&state), "Spac Tall");

        let state = Some(XmonadModuleState {
            layout: "Tabbed Simplest".to_owned(),
        });
        assert_eq!(module.render(&state), "Tabb Simp");

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
