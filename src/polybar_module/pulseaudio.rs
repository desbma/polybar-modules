use std::{
    fs,
    io::{self, BufRead, Read},
    os::unix::fs::PermissionsExt as _,
    process::{Child, Command, Stdio},
    thread::sleep,
    time::Duration,
};

use anyhow::Context;

use crate::{markup, polybar_module::RenderablePolybarModule, theme};

pub(crate) struct PulseAudioModule {
    pactl_subscribe_child: Child,
    easyeffects_installed: bool,
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
pub(crate) struct PulseAudioModuleState {
    sources: Vec<PulseAudioSource>,
    sinks: Vec<PulseAudioSink>,
    easyeffects: Option<bool>,
}

fn easyeffects_installed() -> bool {
    fs::metadata("/usr/bin/easyeffects")
        .ok()
        .is_some_and(|p| (p.permissions().mode() & 0o001) != 0)
}

fn is_systemd_user_unit_running(name: &str) -> bool {
    Command::new("systemctl")
        .args(["--user", "-q", "is-active", name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

impl PulseAudioModule {
    pub(crate) fn new() -> anyhow::Result<Self> {
        // Pactl process to follow events
        let child = Self::subscribe()?;
        let easyeffects_installed = easyeffects_installed();

        Ok(Self {
            pactl_subscribe_child: child,
            easyeffects_installed,
        })
    }

    fn subscribe() -> io::Result<Child> {
        Command::new("pactl")
            .args(["subscribe"]) // LANG=C has no effect on this one
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
    }

    #[expect(clippy::too_many_lines)]
    fn try_update(&mut self) -> anyhow::Result<PulseAudioModuleState> {
        // Run pactl
        let output_sources = Command::new("pactl")
            .args(["list", "sources"])
            .env("LANG", "C")
            .stderr(Stdio::null())
            .output()?;
        output_sources
            .status
            .exit_ok()
            .context("pactl exited with error")?;

        // Parse output
        let mut output_sources_lines = output_sources
            .stdout
            .lines()
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|l| l.trim().to_owned());
        let mut sources = Vec::new();
        loop {
            let source_lines: Vec<_> = output_sources_lines
                .by_ref()
                .skip_while(|l| !l.starts_with("Source #"))
                .take_while(|l| !l.is_empty())
                .collect();
            match source_lines.iter().find(|l| l.starts_with("Source #")) {
                None => break,
                Some(source_id_line) => {
                    let id = source_id_line
                        .rsplit('#')
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("Failed to parse pactl source id"))?
                        .parse()?;
                    let running = source_lines
                        .iter()
                        .find(|l| l.starts_with("State: "))
                        .ok_or_else(|| anyhow::anyhow!("Failed to parse pactl source state"))?
                        .ends_with("RUNNING");
                    if !source_lines
                        .iter()
                        .find(|l| l.starts_with("device.class = "))
                        .is_some_and(|l| l.ends_with("\"sound\""))
                        && !source_lines
                            .iter()
                            .find(|l| l.starts_with("media.class = "))
                            .is_some_and(|l| l.ends_with("\"Audio/Source\""))
                    {
                        // Not a real device
                        continue;
                    }
                    let name = source_lines
                        .iter()
                        .find(|l| {
                            l.starts_with("alsa.card_name = ")
                                || l.starts_with("bluez.alias = ")
                                || l.starts_with("device.alias = ")
                        })
                        .and_then(|s| s.split('"').nth(1))
                        .ok_or_else(|| anyhow::anyhow!("Failed to parse pactl source name"))?
                        .to_owned();
                    sources.push(PulseAudioSource {
                        id,
                        name: Self::abbrev(&name, 1),
                        running,
                    });
                }
            }
        }

        // Run pactl
        let output_sinks = Command::new("pactl")
            .args(["list", "sinks"])
            .env("LANG", "C")
            .stderr(Stdio::null())
            .output()?;
        output_sinks
            .status
            .exit_ok()
            .context("pactl exited with error")?;

        // Parse output
        let mut output_sink_lines = output_sinks
            .stdout
            .lines()
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|l| l.trim().to_owned());
        let mut sinks = Vec::new();
        loop {
            let sink_lines: Vec<_> = output_sink_lines
                .by_ref()
                .skip_while(|l| !l.starts_with("Sink #"))
                .take_while(|l| !l.is_empty())
                .collect();
            match sink_lines.iter().find(|l| l.starts_with("Sink #")) {
                None => break,
                Some(sink_id_line) => {
                    let id = sink_id_line
                        .rsplit('#')
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("Failed to parse pactl sink id"))?
                        .parse()?;
                    let running = sink_lines
                        .iter()
                        .find(|l| l.starts_with("State: "))
                        .ok_or_else(|| anyhow::anyhow!("Failed to parse pactl sink state"))?
                        .ends_with("RUNNING");
                    if !sink_lines
                        .iter()
                        .find(|l| l.starts_with("device.class = "))
                        .is_some_and(|l| l.ends_with("\"sound\""))
                        && !sink_lines
                            .iter()
                            .find(|l| l.starts_with("media.class = "))
                            .is_some_and(|l| l.ends_with("\"Audio/Sink\""))
                    {
                        // Not a real device
                        continue;
                    }
                    let Some(name) = sink_lines
                        .iter()
                        .find(|l| {
                            l.starts_with("alsa.card_name = ")
                                || l.starts_with("bluez.alias = ")
                                || l.starts_with("device.alias = ")
                        })
                        .map(|s| {
                            s.split('"')
                                .nth(1)
                                .map(str::to_owned)
                                .ok_or_else(|| anyhow::anyhow!("Failed to parse pactl sink name"))
                        })
                        .transpose()?
                    else {
                        continue;
                    };
                    sinks.push(PulseAudioSink {
                        id,
                        name: Self::abbrev(&name, 1),
                        running,
                    });
                }
            }
        }
        let easyeffects = self
            .easyeffects_installed
            .then(|| is_systemd_user_unit_running("easyeffects.service"));

        Ok(PulseAudioModuleState {
            sources,
            sinks,
            easyeffects,
        })
    }

    fn abbrev(s: &str, max_len: usize) -> String {
        assert!(max_len >= 1);
        let mut longest_word = s.split(' ').max_by_key(|w| w.len()).unwrap().to_owned();
        if longest_word.len() > max_len {
            if max_len > 1 {
                longest_word.truncate(max_len - 1);
                format!("{longest_word}…")
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
    fn drop(&mut self) {
        let _ = self.pactl_subscribe_child.kill();
    }
}

impl RenderablePolybarModule for PulseAudioModule {
    type State = Option<PulseAudioModuleState>;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        if prev_state.is_some() {
            let mut buffer = vec![0; 65536];
            loop {
                // Read new data
                let read_count = self
                    .pactl_subscribe_child
                    .stdout
                    .as_mut()
                    .unwrap()
                    .read(&mut buffer)
                    .unwrap();
                if read_count == 0 {
                    // pactl subscribe died (can happen when we connect a bluetooth headset)
                    self.pactl_subscribe_child.wait().unwrap();
                    if let Ok(child) = Self::subscribe() {
                        self.pactl_subscribe_child = child;
                    } else {
                        sleep(Duration::from_secs(1));
                    }
                    break;
                }
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
                if let Some(easyeffects) = state.easyeffects {
                    let fragment = markup::action(
                        &markup::style(
                            "󰷞",
                            None,
                            easyeffects.then_some(theme::Color::Foreground),
                            None,
                            None,
                        ),
                        markup::PolybarAction {
                            type_: markup::PolybarActionType::ClickLeft,
                            // Note: starting or stopping easyeffects will trigger a pactl subscribe event,
                            // which will naturally update module to reflect service status
                            command: if easyeffects {
                                "systemctl --user -q --no-block stop easyeffects.service".to_owned()
                            } else {
                                "systemctl --user -q --no-block start easyeffects.service"
                                    .to_owned()
                            },
                        },
                    );
                    fragments.push(fragment);
                }
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
                                    command: format!("pactl set-default-sink {}", sink.id),
                                },
                            )
                        });
                    }
                    fragments.push(String::new());
                } else {
                    fragments.push(" ".to_owned());
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
                                    command: format!("pactl set-default-source {}", source.id),
                                },
                            )
                        });
                    }
                }
                fragments.join(" ").trim_end().to_owned()
            }
            None => markup::style("", Some(theme::Color::Attention), None, None, None),
        }
    }
}

#[cfg(test)]
#[expect(clippy::shadow_unrelated, clippy::too_many_lines)]
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
                    name: "so1".to_owned(),
                    running: false,
                },
                PulseAudioSource {
                    id: 2,
                    name: "so2".to_owned(),
                    running: true,
                },
            ],
            sinks: vec![
                PulseAudioSink {
                    id: 1,
                    name: "si1".to_owned(),
                    running: false,
                },
                PulseAudioSink {
                    id: 2,
                    name: "si2".to_owned(),
                    running: true,
                },
            ],
            easyeffects: None,
        });
        assert_eq!(
            module.render(&state),
            "%{A1:pactl set-default-sink 1:}si1%{A} %{u#93a1a1}%{+u}si2%{-u}  %{F#eee8d5}%{F-} %{A1:pactl set-default-source 1:}so1%{A} %{u#93a1a1}%{+u}so2%{-u}"
        );

        let state = Some(PulseAudioModuleState {
            sources: vec![
                PulseAudioSource {
                    id: 1,
                    name: "so1".to_owned(),
                    running: false,
                },
                PulseAudioSource {
                    id: 2,
                    name: "so2".to_owned(),
                    running: true,
                },
            ],
            sinks: vec![],
            easyeffects: None,
        });
        assert_eq!(
            module.render(&state),
            "  %{F#eee8d5}%{F-} %{A1:pactl set-default-source 1:}so1%{A} %{u#93a1a1}%{+u}so2%{-u}"
        );

        let state = Some(PulseAudioModuleState {
            sources: vec![],
            sinks: vec![
                PulseAudioSink {
                    id: 1,
                    name: "si1".to_owned(),
                    running: false,
                },
                PulseAudioSink {
                    id: 2,
                    name: "si2".to_owned(),
                    running: true,
                },
            ],
            easyeffects: None,
        });
        assert_eq!(
            module.render(&state),
            "%{A1:pactl set-default-sink 1:}si1%{A} %{u#93a1a1}%{+u}si2%{-u}"
        );

        let state = Some(PulseAudioModuleState {
            sources: vec![PulseAudioSource {
                id: 1,
                name: "so1".to_owned(),
                running: false,
            }],
            sinks: vec![PulseAudioSink {
                id: 1,
                name: "si1".to_owned(),
                running: false,
            }],
            easyeffects: None,
        });
        assert_eq!(module.render(&state), "");

        let state = Some(PulseAudioModuleState {
            sources: vec![],
            sinks: vec![],
            easyeffects: Some(true),
        });
        assert_eq!(
            module.render(&state),
            "%{A1:systemctl --user -q --no-block stop easyeffects.service:}%{u#93a1a1}%{+u}\u{f0dde}%{-u}%{A}"
        );

        let state = Some(PulseAudioModuleState {
            sources: vec![],
            sinks: vec![],
            easyeffects: Some(false),
        });
        assert_eq!(
            module.render(&state),
            "%{A1:systemctl --user -q --no-block start easyeffects.service:}\u{f0dde}%{A}"
        );

        let state = Some(PulseAudioModuleState {
            sources: vec![
                PulseAudioSource {
                    id: 1,
                    name: "so1".to_owned(),
                    running: false,
                },
                PulseAudioSource {
                    id: 2,
                    name: "so2".to_owned(),
                    running: true,
                },
            ],
            sinks: vec![],
            easyeffects: Some(true),
        });
        assert_eq!(
            module.render(&state),
            "%{A1:systemctl --user -q --no-block stop easyeffects.service:}%{u#93a1a1}%{+u}\u{f0dde}%{-u}%{A}   %{F#eee8d5}\u{e992}%{F-} %{A1:pactl set-default-source 1:}so1%{A} %{u#93a1a1}%{+u}so2%{-u}"
        );

        let state = Some(PulseAudioModuleState {
            sources: vec![],
            sinks: vec![
                PulseAudioSink {
                    id: 1,
                    name: "si1".to_owned(),
                    running: false,
                },
                PulseAudioSink {
                    id: 2,
                    name: "si2".to_owned(),
                    running: true,
                },
            ],
            easyeffects: Some(true),
        });
        assert_eq!(
            module.render(&state),
            "%{A1:systemctl --user -q --no-block stop easyeffects.service:}%{u#93a1a1}%{+u}\u{f0dde}%{-u}%{A} %{A1:pactl set-default-sink 1:}si1%{A} %{u#93a1a1}%{+u}si2%{-u}"
        );

        let state = Some(PulseAudioModuleState {
            sources: vec![
                PulseAudioSource {
                    id: 1,
                    name: "so1".to_owned(),
                    running: false,
                },
                PulseAudioSource {
                    id: 2,
                    name: "so2".to_owned(),
                    running: true,
                },
            ],
            sinks: vec![
                PulseAudioSink {
                    id: 1,
                    name: "si1".to_owned(),
                    running: false,
                },
                PulseAudioSink {
                    id: 2,
                    name: "si2".to_owned(),
                    running: true,
                },
            ],
            easyeffects: Some(true),
        });
        assert_eq!(
            module.render(&state),
            "%{A1:systemctl --user -q --no-block stop easyeffects.service:}%{u#93a1a1}%{+u}\u{f0dde}%{-u}%{A} %{A1:pactl set-default-sink 1:}si1%{A} %{u#93a1a1}%{+u}si2%{-u}  %{F#eee8d5}\u{e992}%{F-} %{A1:pactl set-default-source 1:}so1%{A} %{u#93a1a1}%{+u}so2%{-u}"
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
