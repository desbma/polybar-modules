use std::io::BufRead;
use std::io::Read;
use std::process::{Child, Command, Stdio};

use anyhow::Context;

use crate::markup;
use crate::polybar_module::RenderablePolybarModule;
use crate::theme;

pub struct PulseAudioModule {
    pactl_subscribe_child: Child,
}

#[derive(Debug, Eq, PartialEq)]
struct PulseAudioSource {
    id: u32,
    name: String,
    running: bool,
}

#[derive(Debug, Eq, PartialEq)]
struct PulseAudioSink {
    id: u32,
    name: String,
    running: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PulseAudioModuleState {
    sources: Vec<PulseAudioSource>,
    sinks: Vec<PulseAudioSink>,
}

impl PulseAudioModule {
    pub fn new() -> anyhow::Result<Self> {
        // Pactl process to follow events
        let child = Command::new("pactl")
            .args(["subscribe"]) // LANG=C has no effect on this one
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        Ok(Self {
            pactl_subscribe_child: child,
        })
    }

    fn try_update(&mut self) -> anyhow::Result<PulseAudioModuleState> {
        // Run pactl
        let output = Command::new("pactl")
            .args(["list", "sources"])
            .env("LANG", "C")
            .stderr(Stdio::null())
            .output()?;
        output.status.exit_ok().context("pactl exited with error")?;

        // Parse output
        let mut output_lines = output
            .stdout
            .lines()
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|l| l.trim().to_owned());
        let mut sources = Vec::new();
        let parse_err_str = "Failed to parse pactl output";
        loop {
            match output_lines.find(|l| l.starts_with("Source #")) {
                None => break,
                Some(source_line) => {
                    let id = source_line
                        .rsplit('#')
                        .next()
                        .ok_or_else(|| anyhow::anyhow!(parse_err_str))?
                        .parse()?;
                    let running = output_lines
                        .find(|l| l.starts_with("State: "))
                        .ok_or_else(|| anyhow::anyhow!(parse_err_str))?
                        .ends_with("RUNNING");
                    if !output_lines
                        .find(|l| l.starts_with("device.class = "))
                        .map(|l| l.ends_with("\"sound\""))
                        .unwrap_or(false)
                    {
                        // Not a real device
                        continue;
                    }
                    let name = output_lines
                        .find(|l| {
                            l.starts_with("alsa.card_name = ") || l.starts_with("bluez.alias = ")
                        })
                        .ok_or_else(|| anyhow::anyhow!(parse_err_str))?
                        .split('"')
                        .nth(1)
                        .ok_or_else(|| anyhow::anyhow!(parse_err_str))?
                        .to_string();
                    sources.push(PulseAudioSource {
                        id,
                        name: Self::abbrev(&name, 1),
                        running,
                    });
                }
            }
        }

        // Run pactl
        let output = Command::new("pactl")
            .args(["list", "sinks"])
            .env("LANG", "C")
            .stderr(Stdio::null())
            .output()?;
        output.status.exit_ok().context("pactl exited with error")?;

        // Parse output
        let mut output_lines = output
            .stdout
            .lines()
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|l| l.trim().to_owned());
        let mut sinks = Vec::new();
        loop {
            match output_lines.find(|l| l.starts_with("Sink #")) {
                None => break,
                Some(sink_line) => {
                    let id = sink_line
                        .rsplit('#')
                        .next()
                        .ok_or_else(|| anyhow::anyhow!(parse_err_str))?
                        .parse()?;
                    let running = output_lines
                        .find(|l| l.starts_with("State: "))
                        .ok_or_else(|| anyhow::anyhow!(parse_err_str))?
                        .ends_with("RUNNING");
                    if !output_lines
                        .find(|l| l.starts_with("device.class = "))
                        .map(|l| l.ends_with("\"sound\""))
                        .unwrap_or(false)
                    {
                        // Not a real device
                        continue;
                    }
                    let name = output_lines
                        .find(|l| {
                            l.starts_with("alsa.card_name = ") || l.starts_with("bluez.alias = ")
                        })
                        .ok_or_else(|| anyhow::anyhow!(parse_err_str))?
                        .split('"')
                        .nth(1)
                        .ok_or_else(|| anyhow::anyhow!(parse_err_str))?
                        .to_string();
                    sinks.push(PulseAudioSink {
                        id,
                        name: Self::abbrev(&name, 1),
                        running,
                    });
                }
            }
        }

        Ok(PulseAudioModuleState { sources, sinks })
    }

    fn abbrev(s: &str, max_len: usize) -> String {
        assert!(max_len >= 1);
        let mut longest_word = s.split(' ').max_by_key(|w| w.len()).unwrap().to_owned();
        if longest_word.len() > max_len {
            if max_len > 1 {
                longest_word.truncate(max_len - 1);
                format!("{}…", longest_word)
            } else {
                longest_word.truncate(1);
                longest_word
            }
        } else {
            longest_word
        }
    }
}

impl Drop for PulseAudioModule {
    #[allow(unused_must_use)]
    fn drop(&mut self) {
        self.pactl_subscribe_child.kill();
    }
}

impl RenderablePolybarModule for PulseAudioModule {
    type State = Option<PulseAudioModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if prev_state.is_some() {
            let mut buffer = [0; 65536];
            loop {
                // Read new data
                let read_count = self
                    .pactl_subscribe_child
                    .stdout
                    .as_mut()
                    .unwrap()
                    .read(&mut buffer)
                    .unwrap();
                let read_str = String::from_utf8_lossy(&buffer[0..read_count]);
                log::trace!("{} bytes read: {:?}", read_count, read_str);
                // Ignore events generated by the pactl invocations in try_update
                if read_str.lines().any(|l| !l.contains(" client #")) {
                    break;
                }
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
            Some(state) => {
                let mut fragments: Vec<String> = Vec::new();
                if state.sinks.len() > 1 {
                    for sink in &state.sinks {
                        fragments.push(if sink.running {
                            markup::style(
                                &sink.name,
                                None,
                                Some(theme::Color::Foreground),
                                None,
                                None,
                            )
                        } else {
                            markup::action(
                                &sink.name,
                                markup::PolybarAction {
                                    type_: markup::PolybarActionType::ClickLeft,
                                    command: format!("pacmd set-default-sink {}", sink.id),
                                },
                            )
                        });
                    }
                    fragments.push("".to_string());
                } else {
                    fragments.push(" ".to_string());
                }
                if state.sources.len() > 1 {
                    fragments.push(markup::style(
                        "",
                        Some(theme::Color::MainIcon),
                        None,
                        None,
                        None,
                    ));
                    for source in &state.sources {
                        fragments.push(if source.running {
                            markup::style(
                                &source.name,
                                None,
                                Some(theme::Color::Foreground),
                                None,
                                None,
                            )
                        } else {
                            markup::action(
                                &source.name,
                                markup::PolybarAction {
                                    type_: markup::PolybarActionType::ClickLeft,
                                    command: format!("pacmd set-default-source {}", source.id),
                                },
                            )
                        });
                    }
                }
                fragments.join(" ").trim_end().to_string()
            }
            None => markup::style("", Some(theme::Color::Attention), None, None, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_abbrev() {
        assert_eq!(PulseAudioModule::abbrev("111 2222", 4), "2222");
        assert_eq!(PulseAudioModule::abbrev("111 2222", 3), "22…");
        assert_eq!(PulseAudioModule::abbrev("111 2222", 1), "2");
    }

    #[test]
    fn test_render() {
        let module = PulseAudioModule::new().unwrap();

        let state = Some(PulseAudioModuleState {
            sources: vec![
                PulseAudioSource {
                    id: 1,
                    name: "so1".to_string(),
                    running: false,
                },
                PulseAudioSource {
                    id: 2,
                    name: "so2".to_string(),
                    running: true,
                },
            ],
            sinks: vec![
                PulseAudioSink {
                    id: 1,
                    name: "si1".to_string(),
                    running: false,
                },
                PulseAudioSink {
                    id: 2,
                    name: "si2".to_string(),
                    running: true,
                },
            ],
        });
        assert_eq!(
            module.render(&state),
            "%{A1:pacmd set-default-sink 1:}si1%{A} %{u#93a1a1}%{+u}si2%{-u}  %{F#eee8d5}%{F-} %{A1:pacmd set-default-source 1:}so1%{A} %{u#93a1a1}%{+u}so2%{-u}"
        );

        let state = Some(PulseAudioModuleState {
            sources: vec![
                PulseAudioSource {
                    id: 1,
                    name: "so1".to_string(),
                    running: false,
                },
                PulseAudioSource {
                    id: 2,
                    name: "so2".to_string(),
                    running: true,
                },
            ],
            sinks: vec![],
        });
        assert_eq!(
            module.render(&state),
            "  %{F#eee8d5}%{F-} %{A1:pacmd set-default-source 1:}so1%{A} %{u#93a1a1}%{+u}so2%{-u}"
        );

        let state = Some(PulseAudioModuleState {
            sources: vec![],
            sinks: vec![
                PulseAudioSink {
                    id: 1,
                    name: "si1".to_string(),
                    running: false,
                },
                PulseAudioSink {
                    id: 2,
                    name: "si2".to_string(),
                    running: true,
                },
            ],
        });
        assert_eq!(
            module.render(&state),
            "%{A1:pacmd set-default-sink 1:}si1%{A} %{u#93a1a1}%{+u}si2%{-u}"
        );

        let state = Some(PulseAudioModuleState {
            sources: vec![PulseAudioSource {
                id: 1,
                name: "so1".to_string(),
                running: false,
            }],
            sinks: vec![PulseAudioSink {
                id: 1,
                name: "si1".to_string(),
                running: false,
            }],
        });
        assert_eq!(module.render(&state), "");

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
