use std::fs::metadata;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::thread::sleep;
use std::time::{Duration, SystemTime};

use notify::Watcher;

use crate::markup;
use crate::polybar_module::RenderablePolybarModule;
use crate::theme;

pub struct TaskwarriorModule {
    max_len: Option<usize>,
    data_dir: String,
}

#[derive(Debug, PartialEq)]
pub struct TaskwarriorModuleState {
    pending_count: usize,
    next_task: String,
    next_task_project: Option<String>,
    next_task_urgency: f32,
    last_fs_change: SystemTime,
}

impl TaskwarriorModule {
    pub fn new(max_len: Option<usize>) -> anyhow::Result<TaskwarriorModule> {
        // Run task to get data.location
        let output = Command::new("task")
            .args(&["show", "data.location"])
            .stderr(Stdio::null())
            .output()?;
        if !output.status.success() {
            anyhow::bail!("task invocation failed");
        }

        // Parse output
        let output_str = String::from_utf8_lossy(&output.stdout);
        let data_dir_raw = output_str
            .lines()
            .find(|l| l.starts_with("data.location"))
            .ok_or_else(|| anyhow::anyhow!("Failed to parse task output"))?
            .rsplit(' ')
            .next()
            .ok_or_else(|| anyhow::anyhow!("Failed to parse task output"))?
            .trim();

        let data_dir = shellexpand::tilde(&data_dir_raw).into_owned();
        Ok(TaskwarriorModule { max_len, data_dir })
    }

    fn try_update(&mut self) -> anyhow::Result<TaskwarriorModuleState> {
        let last_fs_change = self.get_max_task_data_file_mtime();
        let common_task_args = &["rc.verbose:nothing", "rc.gc:off", "recurrence.limit=0"];

        // Run task
        let mut args: Vec<&str> = common_task_args.to_vec();
        args.extend(&["status:pending", "count"]);
        log::debug!("task {:?}", args);
        let output = Command::new("task")
            .args(args)
            .stderr(Stdio::null())
            .output()?;
        if !output.status.success() {
            anyhow::bail!("task invocation failed");
        }

        // Parse output
        let pending_count = String::from_utf8_lossy(&output.stdout).trim().parse()?;

        // Run task
        let mut args: Vec<&str> = common_task_args.to_vec();
        args.extend(&[
            "rc.report.next.columns:urgency,description",
            "rc.report.next.labels:",
            "limit:1",
            "next",
        ]);
        log::debug!("task {:?}", args);
        let output = Command::new("task")
            .args(args)
            .stderr(Stdio::null())
            .output()?;
        if !output.status.success() {
            anyhow::bail!("task invocation failed");
        }

        // Parse output
        let output = String::from_utf8_lossy(&output.stdout);
        let mut output_tokens = output.trim().splitn(2, ' ');
        let next_task_urgency = output_tokens.next().unwrap().parse()?;
        let next_task = output_tokens
            .next()
            .ok_or_else(|| anyhow::anyhow!("Failed to parse task output"))?
            .parse()?;

        // Run task
        let mut args: Vec<&str> = common_task_args.to_vec();
        args.extend(&[
            "rc.report.next.columns:project",
            "rc.report.next.labels:",
            "limit:1",
            "next",
        ]);
        log::debug!("task {:?}", args);
        let output = Command::new("task")
            .args(args)
            .stderr(Stdio::null())
            .output()?;
        if !output.status.success() {
            anyhow::bail!("task invocation failed");
        }

        // Parse output
        let next_task_project = match String::from_utf8_lossy(&output.stdout).trim() {
            "" => None,
            s => Some(s.to_string()),
        };

        Ok(TaskwarriorModuleState {
            pending_count,
            next_task,
            next_task_project,
            next_task_urgency,
            last_fs_change,
        })
    }

    fn ellipsis(s: &str, max_len: Option<usize>) -> String {
        match max_len {
            Some(max_len) => {
                if s.len() > max_len {
                    let mut s2: String = s.trim_end().to_string();
                    if s2.len() > max_len {
                        s2.truncate(max_len - 1);
                        s2.push('…');
                    }
                    s2
                } else {
                    s.to_string()
                }
            }
            None => s.to_string(),
        }
    }

    fn get_max_task_data_file_mtime(&self) -> SystemTime {
        vec!["completed.data", "pending.data"]
            .iter()
            .map(|f| Path::new(&self.data_dir).join(f))
            .filter_map(|p| metadata(p).ok())
            .map(|m| m.modified().unwrap())
            .max()
            .unwrap()
    }
}

impl RenderablePolybarModule for TaskwarriorModule {
    type State = Option<TaskwarriorModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if let Some(prev_state) = prev_state {
            match prev_state {
                // Nominal
                Some(prev_state) => {
                    let (events_tx, events_rx) = channel();
                    let mut watcher =
                        notify::watcher(events_tx, Duration::from_millis(10)).unwrap();
                    let to_watch_filepaths: Vec<PathBuf> = vec!["completed.data", "pending.data"]
                        .iter()
                        .map(|f| Path::new(&self.data_dir).join(f))
                        .collect();

                    log::debug!("Watching {:?}", to_watch_filepaths);
                    for to_watch_filepath in to_watch_filepaths {
                        watcher
                            .watch(to_watch_filepath, notify::RecursiveMode::NonRecursive)
                            .unwrap();
                    }
                    loop {
                        let max_mtime = self.get_max_task_data_file_mtime();
                        if max_mtime > prev_state.last_fs_change {
                            break;
                        }

                        let evt = events_rx.recv().unwrap();
                        log::trace!("{:?}", evt);
                    }
                }
                // Error occured
                None => sleep(Duration::from_secs(1)),
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
                let s1 = format!(
                    "{} ",
                    markup::style("", Some(theme::Color::MainIcon), None, None, None)
                );
                let s2 = format!("{} ", state.pending_count);
                let max_project_len = match self.max_len {
                    None => None,
                    Some(max_len) => {
                        if let Some(next_task_project) = &state.next_task_project {
                            if s2.len() + next_task_project.len() + 3 + state.next_task.len()
                                > max_len
                            {
                                Some((max_len - s2.len() - 3) / 3)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                };
                let s3 = match &state.next_task_project {
                    Some(next_task_project) => {
                        format!("[{}] ", Self::ellipsis(&next_task_project, max_project_len))
                    }
                    None => String::new(),
                };
                let max_task_len = self
                    .max_len
                    .map(|max_len| max_len - s2.len() - s3.chars().count());
                let s4 = Self::ellipsis(&state.next_task, max_task_len);
                format!(
                    "{}{}{}",
                    s1,
                    s2,
                    markup::style(
                        &format!("{}{}", s3, s4),
                        None,
                        if state.next_task_urgency > 9.5 {
                            Some(theme::Color::Attention)
                        } else if state.next_task_urgency > 8.5 {
                            Some(theme::Color::Notice)
                        } else if state.next_task_urgency > 7.5 {
                            Some(theme::Color::Foreground)
                        } else {
                            None
                        },
                        None,
                        None
                    )
                )
            }
            None => markup::style("", Some(theme::Color::Attention), None, None, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ellipsis() {
        assert_eq!(
            TaskwarriorModule::ellipsis("blah blah blah", None),
            "blah blah blah"
        );
        assert_eq!(
            TaskwarriorModule::ellipsis("blah blah blah", Some(14)),
            "blah blah blah"
        );
        assert_eq!(
            TaskwarriorModule::ellipsis("blah blah blah!", Some(14)),
            "blah blah bla…"
        );
        assert_eq!(
            TaskwarriorModule::ellipsis("blah blah blah ", Some(14)),
            "blah blah blah"
        );
        assert_eq!(
            TaskwarriorModule::ellipsis("blah blah bla h", Some(14)),
            "blah blah bla…"
        );
    }

    #[test]
    fn test_render() {
        let module = TaskwarriorModule::new(None).unwrap();

        let state = Some(TaskwarriorModuleState {
            pending_count: 10,
            next_task: "todo".to_string(),
            next_task_project: Some("proj".to_string()),
            next_task_urgency: 1.5,
            last_fs_change: SystemTime::now(),
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 10 [proj] todo");

        let state = Some(TaskwarriorModuleState {
            pending_count: 10,
            next_task: "todo".to_string(),
            next_task_project: None,
            next_task_urgency: 1.5,
            last_fs_change: SystemTime::now(),
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 10 todo");

        let state = Some(TaskwarriorModuleState {
            pending_count: 10,
            next_task: "todo".to_string(),
            next_task_project: Some("proj".to_string()),
            next_task_urgency: 7.51,
            last_fs_change: SystemTime::now(),
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 10 %{u#93a1a1}%{+u}[proj] todo%{-u}"
        );

        let state = Some(TaskwarriorModuleState {
            pending_count: 10,
            next_task: "todo".to_string(),
            next_task_project: Some("proj".to_string()),
            next_task_urgency: 8.51,
            last_fs_change: SystemTime::now(),
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 10 %{u#b58900}%{+u}[proj] todo%{-u}"
        );

        let state = Some(TaskwarriorModuleState {
            pending_count: 10,
            next_task: "todo".to_string(),
            next_task_project: Some("proj".to_string()),
            next_task_urgency: 9.51,
            last_fs_change: SystemTime::now(),
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} 10 %{u#cb4b16}%{+u}[proj] todo%{-u}"
        );

        let module = TaskwarriorModule::new(Some(14)).unwrap();

        let state = Some(TaskwarriorModuleState {
            pending_count: 10,
            next_task: "todo".to_string(),
            next_task_project: Some("proj".to_string()),
            next_task_urgency: 1.5,
            last_fs_change: SystemTime::now(),
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 10 [proj] todo");

        let state = Some(TaskwarriorModuleState {
            pending_count: 101,
            next_task: "todo".to_string(),
            next_task_project: Some("proj".to_string()),
            next_task_urgency: 1.5,
            last_fs_change: SystemTime::now(),
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 101 [p…] todo");

        let state = Some(TaskwarriorModuleState {
            pending_count: 1011,
            next_task: "todo".to_string(),
            next_task_project: Some("proj".to_string()),
            next_task_urgency: 1.5,
            last_fs_change: SystemTime::now(),
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 1011 [p…] todo");

        let state = Some(TaskwarriorModuleState {
            pending_count: 10,
            next_task: "todozz".to_string(),
            next_task_project: Some("proj".to_string()),
            next_task_urgency: 1.5,
            last_fs_change: SystemTime::now(),
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 10 [p…] todozz");

        let state = Some(TaskwarriorModuleState {
            pending_count: 10,
            next_task: "todozzz".to_string(),
            next_task_project: Some("proj".to_string()),
            next_task_urgency: 1.5,
            last_fs_change: SystemTime::now(),
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} 10 [p…] todoz…");
    }
}