use std::env;
use std::ffi::OsString;
use std::fs::metadata;
use std::io::BufRead;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::thread::sleep;
use std::time::{Duration, SystemTime};

use notify::Watcher;

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
        first_task: Box<todo_lib::todotxt::Task>,
        last_fs_change: Option<SystemTime>,
    },
    Paused,
}

impl TodoTxtModule {
    pub fn new(max_len: Option<usize>) -> anyhow::Result<TodoTxtModule> {
        // Run bash to get todo.txt path
        let todotxt_str = match env::var_os("TODO_FILE") {
            None => {
                let output = Command::new("bash")
                    .args(["-c", ". ~/.config/todo/config && echo -n ${TODO_FILE}"])
                    .stderr(Stdio::null())
                    .output()?;
                if !output.status.success() {
                    anyhow::bail!("bash invocation failed");
                }
                OsString::from_vec(output.stdout)
            }
            Some(p) => p,
        };
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

                // Run todo.txt to get first task
                // Warning: this only works if default action is 'more ls'
                // and carries our patches to remove relative date additions
                let output = Command::new("todo.sh").stderr(Stdio::null()).output()?;
                if !output.status.success() {
                    anyhow::bail!("todo.sh invocation failed");
                }

                // Parse first line
                let lines = strip_ansi_escapes::strip(output.stdout)?
                    .lines()
                    .collect::<Result<Vec<_>, _>>()?;
                let task_lines: Vec<_> = lines
                    .iter()
                    .flat_map(|l| l.split_once(' ').map(|t| t.1))
                    .collect();
                log::debug!("{task_lines:?}");
                let first_task_line = task_lines
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("Invalid task.sh output or no task"))?;
                let now = chrono::Local::now().date_naive();
                let first_task = todo_lib::todotxt::Task::parse(first_task_line, now);
                log::trace!("{first_task:?}");
                // let pending_count = lines
                //     .last()
                //     .unwrap()
                //     .split(' ')
                //     .nth(1)
                //     .ok_or_else(|| anyhow::anyhow!("Invalid last line"))?
                //     .parse()?;
                let pending_count = task_lines.len();
                Ok(TodoTxtModuleState::Active {
                    pending_count,
                    first_task: Box::new(first_task),
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
                first_task,
                ..
            }) => {
                let s1 = format!(
                    "{} ",
                    markup::style("", Some(theme::Color::MainIcon), None, None, None)
                );
                let s2 = format!("{} ", pending_count);
                let max_task_len = self.max_len.map(|max_len| max_len - s2.len());
                let task_str = first_task
                    .subject
                    .split(' ')
                    .filter(|w| {
                        !w.starts_with('+') && !w.starts_with("t:") && !w.starts_with("due:")
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let s3 = theme::ellipsis(&task_str, max_task_len);
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
                                match first_task.priority {
                                    0 | 1 => Some(theme::Color::Attention),
                                    2 => Some(theme::Color::Notice),
                                    3 => Some(theme::Color::Foreground),
                                    _ => None,
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

    use todo_lib::todotxt::Task;

    #[test]
    fn test_render() {
        let xdg_dirs = xdg::BaseDirectories::new().unwrap();
        let runtime_dir = xdg_dirs.get_runtime_directory().unwrap();
        let module = TodoTxtModule::new(None).unwrap();

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            first_task: Box::new(Task {
                subject: "todo".to_string(),
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
            first_task: Box::new(Task {
                subject: "todo".to_string(),
                priority: 3,
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
            first_task: Box::new(Task {
                subject: "todo".to_string(),
                priority: 2,
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#eee8d5}}%{{F-}} %{{A1:touch {}/public_screen:}}10 %{{u#b58900}}%{{+u}}todo%{{-u}}%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            first_task: Box::new(Task {
                subject: "todo".to_string(),
                priority: 0,
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
            first_task: Box::new(Task {
                subject: "todo".to_string(),
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
            first_task: Box::new(Task {
                subject: "todo".to_string(),
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
            first_task: Box::new(Task {
                subject: "todo".to_string(),
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
            first_task: Box::new(Task {
                subject: "todozzz".to_string(),
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
