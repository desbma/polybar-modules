use std::env;
use std::fs::metadata;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::str;
use std::sync::mpsc::channel;
use std::thread::sleep;
use std::time::{Duration, SystemTime};

use anyhow::Context;
use notify::Watcher;
use tasks::Task;

use crate::markup;
use crate::polybar_module::{PolybarModuleEnv, RenderablePolybarModule};
use crate::theme;

pub struct TodoTxtModule {
    max_len: Option<usize>,
    todotxt_filepath: PathBuf,
    env: PolybarModuleEnv,
}

#[derive(Debug, PartialEq)]
pub enum TodoTxtModuleState {
    Active {
        pending_count: usize,
        next_task: Option<Task>,
        last_fs_change: Option<SystemTime>,
    },
    Paused,
}

impl TodoTxtModule {
    pub fn new(max_len: Option<usize>) -> anyhow::Result<TodoTxtModule> {
        let todotxt_str = env::var_os("TODO_FILE")
            .ok_or_else(|| anyhow::anyhow!("TODO_FILE environment variable is not set"))?;
        let todotxt_filepath = PathBuf::from(todotxt_str);
        log::debug!("todo.txt path: {todotxt_filepath:?}");
        let env = PolybarModuleEnv::new();

        Ok(TodoTxtModule {
            max_len,
            todotxt_filepath,
            env,
        })
    }

    fn try_update(&mut self) -> anyhow::Result<TodoTxtModuleState> {
        match self.env.public_screen() {
            false => {
                let last_fs_change = self.get_todotxt_file_mtime();

                // Run todo to get first task
                let output = Command::new("todo")
                    .arg("next")
                    .stderr(Stdio::null())
                    .output()?;
                output.status.exit_ok().context("todo exited with error")?;

                // Parse task
                let task_str = str::from_utf8(&output.stdout)?.trim_end();
                let task = if task_str.is_empty() {
                    None
                } else {
                    Some(task_str.parse()?)
                };
                log::trace!("{task:?}");

                // Get pending count
                let output = Command::new("todo")
                    .arg("pending-count")
                    .stderr(Stdio::null())
                    .output()?;
                output.status.exit_ok().context("todo exited with error")?;
                let pending_count = str::from_utf8(&output.stdout)?.trim_end().parse()?;

                Ok(TodoTxtModuleState::Active {
                    pending_count,
                    next_task: task,
                    last_fs_change,
                })
            }
            true => Ok(TodoTxtModuleState::Paused),
        }
    }

    fn get_todotxt_file_mtime(&self) -> Option<SystemTime> {
        metadata(&self.todotxt_filepath)
            .ok()
            .map(|m| m.modified().unwrap())
    }
}

impl RenderablePolybarModule for TodoTxtModule {
    type State = Option<TodoTxtModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
        if let Some(prev_state) = prev_state {
            match prev_state {
                // Nominal
                Some(TodoTxtModuleState::Active { last_fs_change, .. }) => {
                    let (events_tx, events_rx) = channel();
                    let mut watcher =
                        notify::watcher(events_tx, Duration::from_millis(10)).unwrap();
                    let to_watch_filepaths = [
                        &self.todotxt_filepath,
                        self.env.public_screen_filepath.parent().unwrap(),
                    ];

                    log::debug!("Watching {:?}", to_watch_filepaths);
                    for to_watch_filepath in to_watch_filepaths {
                        watcher
                            .watch(to_watch_filepath, notify::RecursiveMode::NonRecursive)
                            .unwrap();
                    }
                    while !self.env.public_screen() {
                        let max_mtime = self.get_todotxt_file_mtime();
                        if max_mtime != *last_fs_change {
                            break;
                        }

                        let evt = events_rx.recv().unwrap();
                        log::trace!("{:?}", evt);
                    }
                }
                Some(TodoTxtModuleState::Paused) => {
                    self.env.wait_public_screen(false);
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
            Some(TodoTxtModuleState::Active {
                pending_count,
                next_task,
                ..
            }) => {
                let s1 = format!(
                    "{} ",
                    markup::style("", Some(theme::Color::MainIcon), None, None, None)
                );
                let s2 = format!("{} ", pending_count);
                let max_task_len = self.max_len.map(|max_len| max_len - s2.len());
                let s3 = if let Some(task) = next_task {
                    theme::ellipsis(&task.text, max_task_len)
                } else {
                    "😌".to_string()
                };
                format!(
                    "{}{}",
                    s1,
                    markup::action(
                        &format!(
                            "{}{}",
                            s2,
                            markup::style(
                                &s3,
                                None,
                                if next_task
                                    .as_ref()
                                    .and_then(|t| t.due_date())
                                    .map(|d| d <= chrono::Local::now().date_naive())
                                    .unwrap_or(false)
                                {
                                    Some(theme::Color::Attention)
                                } else {
                                    match next_task.as_ref().and_then(|t| t.priority) {
                                        Some('A') => Some(theme::Color::Attention),
                                        Some('B') => Some(theme::Color::Notice),
                                        Some('C') => Some(theme::Color::Foreground),
                                        _ => None,
                                    }
                                },
                                None,
                                None
                            )
                        ),
                        markup::PolybarAction {
                            type_: markup::PolybarActionType::ClickLeft,
                            command: format!(
                                "touch {}",
                                self.env.public_screen_filepath.to_str().unwrap()
                            ),
                        },
                    ),
                )
            }
            Some(TodoTxtModuleState::Paused) => {
                format!(
                    "{} {}",
                    markup::style("", Some(theme::Color::MainIcon), None, None, None),
                    markup::action(
                        &markup::style("", None, None, None, None),
                        markup::PolybarAction {
                            type_: markup::PolybarActionType::ClickLeft,
                            command: format!(
                                "rm {}",
                                self.env.public_screen_filepath.to_str().unwrap()
                            ),
                        },
                    ),
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
    fn test_render() {
        let xdg_dirs = xdg::BaseDirectories::new().unwrap();
        let runtime_dir = xdg_dirs.get_runtime_directory().unwrap();
        let module = TodoTxtModule::new(None).unwrap();

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            next_task: None,
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#eee8d5}}%{{F-}} %{{A1:touch {}/public_screen:}}10 😌%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            next_task: Some(Task {
                priority: None,
                text: "todo".to_string(),
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#eee8d5}}%{{F-}} %{{A1:touch {}/public_screen:}}10 todo%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            next_task: Some(Task {
                priority: Some('D'),
                text: "todo".to_string(),
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#eee8d5}}%{{F-}} %{{A1:touch {}/public_screen:}}10 todo%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            next_task: Some(Task {
                priority: Some('D'),
                text: "todo".to_string(),
                attributes: vec![(
                    "due".to_string(),
                    chrono::Local::now()
                        .date_naive()
                        .format("%Y-%m-%d")
                        .to_string(),
                )],
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#eee8d5}}%{{F-}} %{{A1:touch {}/public_screen:}}10 %{{u#cb4b16}}%{{+u}}todo%{{-u}}%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            next_task: Some(Task {
                priority: Some('C'),
                text: "todo".to_string(),
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#eee8d5}}%{{F-}} %{{A1:touch {}/public_screen:}}10 %{{u#93a1a1}}%{{+u}}todo%{{-u}}%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            next_task: Some(Task {
                priority: Some('A'),
                text: "todo".to_string(),
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#eee8d5}}%{{F-}} %{{A1:touch {}/public_screen:}}10 %{{u#cb4b16}}%{{+u}}todo%{{-u}}%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let module = TodoTxtModule::new(Some(7)).unwrap();

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            next_task: Some(Task {
                priority: None,
                text: "todo".to_string(),
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#eee8d5}}%{{F-}} %{{A1:touch {}/public_screen:}}10 todo%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 101,
            next_task: Some(Task {
                priority: None,
                text: "todo".to_string(),
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#eee8d5}}%{{F-}} %{{A1:touch {}/public_screen:}}101 to…%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 1011,
            next_task: Some(Task {
                priority: None,
                text: "todo".to_string(),
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#eee8d5}}%{{F-}} %{{A1:touch {}/public_screen:}}1011 t…%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            next_task: Some(Task {
                priority: None,
                text: "todozzz".to_string(),
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#eee8d5}}%{{F-}} %{{A1:touch {}/public_screen:}}10 tod…%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Paused);
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#eee8d5}}%{{F-}} %{{A1:rm {}/public_screen:}}%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );
    }
}
