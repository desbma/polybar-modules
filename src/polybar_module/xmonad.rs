use std::{
    collections::HashMap, fs, iter, path::PathBuf, str::FromStr, thread::sleep, time::Duration,
};

use anyhow::Context as _;
use itertools::Itertools as _;

use crate::{
    config::XmonadModuleConfig,
    markup,
    polybar_module::{FilesystemWatcher, RenderablePolybarModule, watch_paths},
    theme::{self, ICON_WARNING},
};

/// Label of the coding session workspaces, before their count
const ICON_CODING_SESSIONS: &str = "󰐣";

pub(crate) struct XmonadModule {
    status_dir: PathBuf,
    /// Monitor of the bar, or `None` for the single monitor of the status
    bar_monitor: Option<String>,
    workspace_icons: HashMap<String, String>,
    watcher: Option<FilesystemWatcher>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct XmonadModuleState {
    labels: Vec<WorkspaceLabel>,
    /// Layout of the workspace on the bar's monitor
    layout: String,
}

#[derive(Debug, PartialEq)]
struct WorkspaceLabel {
    text: String,
    state: WorkspaceState,
}

/// Display state of a shown workspace, in increasing precedence
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum WorkspaceState {
    Occupied,
    /// On the bar's monitor, its screen not focused
    Visible,
    /// On the bar's monitor, its screen focused
    Focused,
    Urgent,
}

/// Status file written by xmonad, the API contract between both: workspaces in display order, split around the focused one
#[derive(serde::Deserialize)]
#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
struct XmonadStatus {
    before: Vec<Workspace<Option<Screen>>>,
    focused: Workspace<Screen>,
    after: Vec<Workspace<Option<Screen>>>,
}

/// Xmonad workspace with the screen showing it
#[derive(serde::Deserialize)]
#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
struct Workspace<S> {
    #[serde(flatten)]
    info: WorkspaceInfo,
    screen: S,
}

#[derive(serde::Deserialize)]
#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
struct WorkspaceInfo {
    tag: String,
    coding_session: bool,
    /// `None` for a workspace without windows
    windows: Option<WorkspaceWindows>,
}

/// Monitor showing a workspace, with the workspace's layout
#[derive(serde::Deserialize)]
#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
struct Screen {
    monitor: String,
    layout: String,
}

#[derive(Clone, Copy, serde::Deserialize)]
#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
struct WorkspaceWindows {
    /// At least one window is urgent
    urgent: bool,
}

impl FromStr for XmonadStatus {
    type Err = anyhow::Error;

    /// Parse a JSON status, rejecting a monitor showing several workspaces
    fn from_str(s: &str) -> anyhow::Result<Self> {
        let status: Self = serde_json::from_str(s)?;
        if let Some(monitor) = status.monitors().duplicates().next() {
            anyhow::bail!("Monitor {monitor:?} shows several workspaces");
        }
        Ok(status)
    }
}

impl XmonadStatus {
    /// Iterate over workspaces in display order, with their screen
    fn workspaces(&self) -> impl Iterator<Item = (&WorkspaceInfo, Option<&Screen>)> {
        self.before
            .iter()
            .map(|w| (&w.info, w.screen.as_ref()))
            .chain(iter::once((&self.focused.info, Some(&self.focused.screen))))
            .chain(self.after.iter().map(|w| (&w.info, w.screen.as_ref())))
    }

    /// Iterate over the monitors of shown workspaces, in display order
    fn monitors(&self) -> impl Iterator<Item = &str> {
        self.workspaces()
            .filter_map(|(_, screen)| screen.map(|s| s.monitor.as_str()))
    }
}

impl XmonadModule {
    pub(crate) fn new(
        bar_monitor: Option<String>,
        cfg: XmonadModuleConfig,
    ) -> anyhow::Result<Self> {
        let status_dir = xdg::BaseDirectories::new()
            .get_runtime_directory()?
            .join("xmonad");
        Ok(Self {
            status_dir,
            bar_monitor: bar_monitor.filter(|m| !m.is_empty()),
            workspace_icons: cfg.workspace_icons,
            watcher: None,
        })
    }

    fn try_update(&mut self) -> anyhow::Result<XmonadModuleState> {
        if self.watcher.is_none() {
            self.watcher = Some(
                watch_paths(&[&self.status_dir])
                    .with_context(|| format!("Failed to watch {dir:?}", dir = self.status_dir))?,
            );
        }

        let status_filepath = self.status_dir.join("status.json");
        let json = fs::read_to_string(&status_filepath)
            .with_context(|| format!("Failed to read {status_filepath:?}"))?;
        self.state(&json.parse()?)
    }

    fn state(&self, status: &XmonadStatus) -> anyhow::Result<XmonadModuleState> {
        let bar_monitor = if let Some(bar_monitor) = &self.bar_monitor {
            bar_monitor.as_str()
        } else {
            let monitors: Vec<_> = status.monitors().collect();
            let [monitor] = monitors.as_slice() else {
                anyhow::bail!("Expected a single monitor without a bar monitor, got {monitors:?}");
            };
            *monitor
        };
        let bar_screen = status
            .workspaces()
            .find_map(|(_, screen)| screen.filter(|s| s.monitor == bar_monitor))
            .ok_or_else(|| anyhow::anyhow!("No workspace on monitor {bar_monitor:?}"))?;
        let bar_state = if status.focused.screen.monitor == bar_monitor {
            WorkspaceState::Focused
        } else {
            WorkspaceState::Visible
        };

        let workspaces: Vec<_> = status
            .workspaces()
            .map(|(workspace, screen)| {
                let state = if workspace.windows.is_some_and(|w| w.urgent) {
                    Some(WorkspaceState::Urgent)
                } else if screen.is_some_and(|s| s.monitor == bar_monitor) {
                    Some(bar_state)
                } else {
                    workspace.windows.map(|_| WorkspaceState::Occupied)
                };
                (workspace, state)
            })
            .collect();

        let coding_sessions = workspaces.iter().filter(|(w, _)| w.coding_session);
        let coding_session_count = coding_sessions
            .clone()
            .filter(|(w, _)| w.windows.is_some())
            .count();
        let mut coding_sessions_label =
            coding_sessions
                .filter_map(|(_, state)| *state)
                .max()
                .map(|state| WorkspaceLabel {
                    text: format!("{ICON_CODING_SESSIONS}{coding_session_count}"),
                    state,
                });
        let labels = workspaces
            .iter()
            .filter_map(|(workspace, state)| {
                if workspace.coding_session {
                    coding_sessions_label.take()
                } else {
                    state.map(|state| WorkspaceLabel {
                        text: self
                            .workspace_icons
                            .get(&workspace.tag)
                            .unwrap_or(&workspace.tag)
                            .clone(),
                        state,
                    })
                }
            })
            .collect();

        Ok(XmonadModuleState {
            labels,
            layout: bar_screen.layout.clone(),
        })
    }
}

/// Abbreviate a layout name to the initials of its words but the last, then the last word cut to 4 characters, words being split at spaces and before capitals
fn abbreviate_layout(layout: &str) -> String {
    let mut label = String::new();
    let mut word = String::new();
    for c in layout.chars() {
        if c == ' ' || c.is_uppercase() {
            label.extend(word.chars().next());
            word.clear();
        }
        if c != ' ' {
            word.push(c);
        }
    }
    label.extend(word.chars().take(4));
    label
}

impl RenderablePolybarModule for XmonadModule {
    type State = Option<XmonadModuleState>;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        match prev_state {
            Some(Some(_)) => {
                let (_, events_rx) = self.watcher.as_ref().unwrap();
                // Skip the access events of the module's own reads
                let evt = events_rx
                    .iter()
                    .find(|evt| !evt.as_ref().is_ok_and(|evt| evt.kind.is_access()))
                    .unwrap();
                log::trace!("{evt:?}");
            }
            Some(None) => sleep(Duration::from_secs(1)),
            None => {}
        }
    }

    fn update(&mut self) -> Self::State {
        match self.try_update() {
            Ok(state) => Some(state),
            Err(e) => {
                log::error!("{e:#}");
                // Watch again, the directory may have been recreated
                self.watcher = None;
                None
            }
        }
    }

    fn render(&self, state: &Self::State) -> String {
        if let Some(state) = state {
            let labels: String = state
                .labels
                .iter()
                .map(|label| {
                    let markup = markup::Markup::new(format!(" {text} ", text = label.text));
                    match label.state {
                        WorkspaceState::Urgent => {
                            markup.fg(theme::Color::MainIcon).bg(theme::Color::Notice)
                        }
                        WorkspaceState::Focused => markup
                            .fg(theme::Color::Background)
                            .bg(theme::Color::Focused),
                        WorkspaceState::Visible => markup
                            .fg(theme::Color::Background)
                            .bg(theme::Color::Unfocused),
                        WorkspaceState::Occupied => markup,
                    }
                    .into_string()
                })
                .collect();
            let layout = abbreviate_layout(&state.layout);
            format!("{labels} {layout}")
        } else {
            markup::Markup::new(ICON_WARNING)
                .fg(theme::Color::Attention)
                .into_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{path::Path, thread};

    use serde_json::{Value, json};

    use super::*;

    const WARNING: &str = "%{F#d56500}\u{f071}%{F-}";

    fn workspace(tag: &str, urgent: Option<bool>, monitor: Option<&str>) -> Value {
        json!({
            "tag": tag,
            "coding_session": tag.starts_with("code:"),
            "windows": urgent.map(|urgent| json!({"urgent": urgent})),
            "screen": monitor.map(|monitor| json!({"monitor": monitor, "layout": "Tall"})),
        })
    }

    fn status(before: &[Value], focused: &Value, after: &[Value]) -> String {
        json!({"before": before, "focused": focused, "after": after}).to_string()
    }

    fn module(status_dir: &Path, bar_monitor: Option<&str>) -> XmonadModule {
        let cfg = XmonadModuleConfig {
            workspace_icons: HashMap::from([
                ("1:NET".to_owned(), "net".to_owned()),
                ("2:DEV".to_owned(), "dev".to_owned()),
            ]),
        };
        XmonadModule {
            status_dir: status_dir.to_owned(),
            ..XmonadModule::new(bar_monitor.map(str::to_owned), cfg).unwrap()
        }
    }

    fn render(bar_monitor: Option<&str>, status: Option<&str>) -> String {
        let status_dir = tempfile::tempdir().unwrap();
        if let Some(status) = status {
            fs::write(status_dir.path().join("status.json"), status).unwrap();
        }
        let mut module = module(status_dir.path(), bar_monitor);
        let state = module.update();
        module.render(&state)
    }

    #[test]
    fn test_parse_status() {
        let json = r#"{"before": [{"tag": "1:NET", "coding_session": false, "windows": {"urgent": false}, "screen": {"monitor": "DVI-I-1", "layout": "Tall"}}],
            "focused": {"tag": "code:~/dev/foo", "coding_session": true, "windows": {"urgent": true}, "screen": {"monitor": "HDMI-0", "layout": "Tabbed Simplest"}},
            "after": [{"tag": "9", "coding_session": false, "windows": null, "screen": null}]}"#;
        assert_eq!(
            json.parse::<XmonadStatus>().unwrap(),
            XmonadStatus {
                before: vec![Workspace {
                    info: WorkspaceInfo {
                        tag: "1:NET".to_owned(),
                        coding_session: false,
                        windows: Some(WorkspaceWindows { urgent: false }),
                    },
                    screen: Some(Screen {
                        monitor: "DVI-I-1".to_owned(),
                        layout: "Tall".to_owned(),
                    }),
                }],
                focused: Workspace {
                    info: WorkspaceInfo {
                        tag: "code:~/dev/foo".to_owned(),
                        coding_session: true,
                        windows: Some(WorkspaceWindows { urgent: true }),
                    },
                    screen: Screen {
                        monitor: "HDMI-0".to_owned(),
                        layout: "Tabbed Simplest".to_owned(),
                    },
                },
                after: vec![Workspace {
                    info: WorkspaceInfo {
                        tag: "9".to_owned(),
                        coding_session: false,
                        windows: None,
                    },
                    screen: None,
                }],
            }
        );
    }

    #[test]
    fn test_render_urgent() {
        let json = status(
            &[workspace("1:NET", Some(true), None)],
            &workspace("2:DEV", Some(true), Some("DP-1")),
            &[],
        );
        assert_eq!(
            render(Some("DP-1"), Some(&json)),
            "%{B#ac8300}%{F#f1e9d2} net %{F-}%{B-}%{B#ac8300}%{F#f1e9d2} dev %{F-}%{B-} Tall"
        );
    }

    #[test]
    fn test_render_focused() {
        let json = status(&[], &workspace("1:NET", None, Some("DP-1")), &[]);
        assert_eq!(
            render(Some("DP-1"), Some(&json)),
            "%{B#259d94}%{F#002d38} net %{F-}%{B-} Tall"
        );
    }

    #[test]
    fn test_render_visible() {
        let mut dev = workspace("2:DEV", None, Some("DP-1"));
        dev["screen"]["layout"] = json!("AutoTall");
        let json = status(&[], &workspace("1:NET", Some(false), Some("DP-2")), &[dev]);
        assert_eq!(
            render(Some("DP-1"), Some(&json)),
            " net %{B#657377}%{F#002d38} dev %{F-}%{B-} ATall"
        );
    }

    #[test]
    fn test_render_occupied_and_empty() {
        let json = status(
            &[],
            &workspace("1:NET", Some(false), Some("DP-1")),
            &[
                workspace("3", Some(false), None),
                workspace("2:DEV", None, None),
            ],
        );
        assert_eq!(
            render(Some("DP-1"), Some(&json)),
            "%{B#259d94}%{F#002d38} net %{F-}%{B-} 3  Tall"
        );
    }

    #[test]
    fn test_render_coding_sessions() {
        let json = status(
            &[
                workspace("1:NET", Some(false), None),
                workspace("code:a", Some(false), None),
            ],
            &workspace("code:b", None, Some("DP-1")),
            &[workspace("code:c", Some(false), None)],
        );
        assert_eq!(
            render(Some("DP-1"), Some(&json)),
            " net %{B#259d94}%{F#002d38} 󰐣2 %{F-}%{B-} Tall"
        );
    }

    #[test]
    fn test_render_coding_sessions_position_and_urgency() {
        let json = status(
            &[
                workspace("code:a", None, None),
                workspace("1:NET", Some(false), None),
            ],
            &workspace("code:b", None, Some("DP-1")),
            &[workspace("code:c", Some(true), None)],
        );
        assert_eq!(
            render(Some("DP-1"), Some(&json)),
            "%{B#ac8300}%{F#f1e9d2} 󰐣1 %{F-}%{B-} net  Tall"
        );
    }

    #[test]
    fn test_render_coding_sessions_empty() {
        let json = status(
            &[],
            &workspace("1:NET", Some(false), Some("DP-1")),
            &[workspace("code:a", None, None)],
        );
        assert_eq!(
            render(Some("DP-1"), Some(&json)),
            "%{B#259d94}%{F#002d38} net %{F-}%{B-} Tall"
        );
    }

    #[test]
    fn test_abbreviate_layout() {
        for (layout, expected) in [
            ("AutoTall", "ATall"),
            ("TmsBinColumn", "TBColu"),
            ("StackTile", "STile"),
            ("BinColumn", "BColu"),
            ("Tabbed", "Tabb"),
            ("2Tabbed", "2Tabb"),
            ("Mirror Tall", "MTall"),
            ("Mirror 2Tabbed", "M2Tabb"),
            ("界界界界界", "界界界界"),
            ("Tall", "Tall"),
            ("Grid", "Grid"),
            ("Full", "Full"),
        ] {
            assert_eq!(abbreviate_layout(layout), expected);
        }
    }

    #[test]
    fn test_render_single_monitor() {
        let json = status(
            &[],
            &workspace("1:NET", None, Some("eDP-1")),
            &[workspace("2:DEV", Some(false), None)],
        );
        let expected = "%{B#259d94}%{F#002d38} net %{F-}%{B-} dev  Tall";
        assert_eq!(render(None, Some(&json)), expected);
        assert_eq!(render(Some(""), Some(&json)), expected);
    }

    #[test]
    fn test_render_errors() {
        assert_eq!(render(Some("DP-1"), None), WARNING);
        assert_eq!(render(Some("DP-1"), Some("{")), WARNING);
        let duplicate_monitor = status(
            &[workspace("1:NET", None, Some("DP-1"))],
            &workspace("2:DEV", None, Some("DP-1")),
            &[],
        );
        assert_eq!(render(Some("DP-1"), Some(&duplicate_monitor)), WARNING);
        let two_monitors = status(
            &[workspace("1:NET", None, Some("DP-2"))],
            &workspace("2:DEV", None, Some("DP-1")),
            &[],
        );
        assert_eq!(render(Some("DP-3"), Some(&two_monitors)), WARNING);
        assert_eq!(render(None, Some(&two_monitors)), WARNING);
    }

    #[test]
    fn test_update_error_resets_watcher() {
        let status_dir = tempfile::tempdir().unwrap();
        fs::write(status_dir.path().join("status.json"), "{").unwrap();
        let mut module = module(status_dir.path(), Some("DP-1"));
        assert_eq!(module.update(), None);
        assert!(module.watcher.is_none());
    }

    #[test]
    fn test_wait_update_ignores_reads() {
        let status_dir = tempfile::tempdir().unwrap();
        let status_filepath = status_dir.path().join("status.json");
        let json = status(&[], &workspace("1:NET", None, Some("DP-1")), &[]);
        fs::write(&status_filepath, &json).unwrap();
        let mut module = module(status_dir.path(), Some("DP-1"));
        let state = module.update();
        thread::scope(|scope| {
            let waiter = scope.spawn(|| module.wait_update(Some(&state)));
            sleep(Duration::from_millis(200));
            assert!(!waiter.is_finished());
            fs::write(&status_filepath, &json).unwrap();
            waiter.join().unwrap();
        });
    }
}
