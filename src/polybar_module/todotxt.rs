use std::{
    env,
    fs::metadata,
    path::PathBuf,
    sync::mpsc::RecvTimeoutError,
    thread::sleep,
    time::{Duration, SystemTime},
};

use jiff::ZonedDateTime;
use tasks::{Task, TodoFile};

use crate::{
    markup,
    polybar_module::{PolybarModuleEnv, RenderablePolybarModule, boottime, watch_paths},
    theme::{self, ICON_WARNING},
};

pub(crate) struct TodoTxtModule {
    max_len: Option<usize>,
    todotxt_filepath: PathBuf,
    done_filepath: PathBuf,
    env: PolybarModuleEnv,
}

#[expect(clippy::large_enum_variant)]
#[derive(Debug, PartialEq)]
pub(crate) enum TodoTxtModuleState {
    Active {
        pending_count: usize,
        next_task: Option<Task>,
        last_fs_change: Option<SystemTime>,
    },
    Paused,
}

impl TodoTxtModule {
    pub(crate) fn new(max_len: Option<usize>) -> anyhow::Result<Self> {
        let todotxt_str = env::var_os("TODO_FILE")
            .ok_or_else(|| anyhow::anyhow!("TODO_FILE environment variable is not set"))?;
        let todotxt_filepath = PathBuf::from(todotxt_str);
        let done_str = env::var_os("DONE_FILE")
            .ok_or_else(|| anyhow::anyhow!("DONE_FILE environment variable is not set"))?;
        let done_filepath = PathBuf::from(done_str);
        let env = PolybarModuleEnv::new();

        Ok(Self {
            max_len,
            todotxt_filepath,
            done_filepath,
            env,
        })
    }

    fn try_update(&mut self) -> anyhow::Result<TodoTxtModuleState> {
        if self.env.public_screen() {
            Ok(TodoTxtModuleState::Paused)
        } else {
            let last_fs_change = self.get_todotxt_file_mtime();

            let today = ZonedDateTime::now().date();
            let task_file = TodoFile::new(&self.todotxt_filepath, &self.done_filepath)?;
            let tasks = task_file.load_tasks()?;
            let ready = tasks.iter().filter(|t| t.is_ready(today, &tasks));
            let pending_count = ready.clone().count();
            let next_task = ready.max_by(|a, b| a.cmp(b, today, &tasks)).cloned();

            Ok(TodoTxtModuleState::Active {
                pending_count,
                next_task,
                last_fs_change,
            })
        }
    }

    fn get_todotxt_file_mtime(&self) -> Option<SystemTime> {
        metadata(&self.todotxt_filepath)
            .ok()
            .map(|m| m.modified().unwrap())
    }
}

const ICON_TODOTXT: &str = "";
const ICON_TODOTXT_HIDDEN: &str = "󰜺";

impl RenderablePolybarModule for TodoTxtModule {
    type State = Option<TodoTxtModuleState>;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        const MAX_WAIT: Duration = Duration::from_hours(1);
        /// Longest a single receive may block, bounding how late a return from suspend is noticed
        const RESUME_CHECK_INTERVAL: Duration = Duration::from_mins(1);
        if let Some(prev_state) = prev_state {
            match prev_state {
                // Nominal
                Some(TodoTxtModuleState::Active { last_fs_change, .. }) => {
                    let (_watcher, events_rx) = watch_paths(&[
                        &self.todotxt_filepath,
                        self.env.public_screen_filepath.parent().unwrap(),
                    ])
                    .unwrap();
                    let deadline = boottime() + MAX_WAIT;
                    while !self.env.public_screen() && boottime() < deadline {
                        let max_mtime = self.get_todotxt_file_mtime();
                        if max_mtime != *last_fs_change {
                            break;
                        }

                        let timeout = deadline
                            .saturating_sub(boottime())
                            .min(RESUME_CHECK_INTERVAL);
                        let res = events_rx.recv_timeout(timeout);
                        let evt = match res {
                            Ok(evt) => evt,
                            // Receiving in slices, the loop condition owns the deadline
                            Err(RecvTimeoutError::Timeout) => continue,
                            Err(_) => res.unwrap(),
                        };
                        log::trace!("{evt:?}");
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
                log::error!("{e}");
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
                    markup::Markup::new(ICON_TODOTXT)
                        .fg(theme::Color::MainIcon)
                        .into_string()
                );
                let s2 = format!("{pending_count} ");
                let max_task_len = self.max_len.map(|max_len| max_len - s2.len());
                let s3 = if let Some(task) = next_task {
                    theme::ellipsis(&task.text, max_task_len)
                } else {
                    "😌".to_owned()
                };
                let color = next_task.as_ref().and_then(|task| {
                    match (task.is_overdue(ZonedDateTime::now().date()), task.priority) {
                        (true, _) | (_, Some('A')) => Some(theme::Color::Attention),
                        (_, Some('B')) => Some(theme::Color::Notice),
                        (_, Some('C')) => Some(theme::Color::Foreground),
                        _ => None,
                    }
                });
                let mut label = markup::Markup::new(s3);
                if let Some(color) = color {
                    label = label.underline(color);
                }
                format!(
                    "{}{}",
                    s1,
                    markup::Markup::new(format!("{s2}{label}", label = label.into_string()))
                        .action(
                            markup::PolybarActionType::ClickLeft,
                            format!(
                                "touch {}",
                                self.env.public_screen_filepath.to_str().unwrap()
                            ),
                        )
                        .into_string(),
                )
            }
            Some(TodoTxtModuleState::Paused) => {
                format!(
                    "{} {}",
                    markup::Markup::new(ICON_TODOTXT)
                        .fg(theme::Color::MainIcon)
                        .into_string(),
                    markup::Markup::new(ICON_TODOTXT_HIDDEN)
                        .action(
                            markup::PolybarActionType::ClickLeft,
                            format!("rm {}", self.env.public_screen_filepath.to_str().unwrap()),
                        )
                        .into_string(),
                )
            }
            None => markup::Markup::new(ICON_WARNING)
                .fg(theme::Color::Attention)
                .into_string(),
        }
    }
}

#[cfg(test)]
#[expect(clippy::shadow_unrelated, clippy::too_many_lines)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn test_try_update() {
        let dir = tempfile::tempdir().unwrap();
        let todo_path = dir.path().join("todo.txt");
        let done_path = dir.path().join("done.txt");
        fs::write(
            &todo_path,
            "(B) ordinary\n(D) overdue due:2000-01-01\n(A) not ready t:2999-01-01\nx 2000-01-01 done\n",
        )
        .unwrap();
        fs::write(&done_path, "").unwrap();
        let mut module = TodoTxtModule {
            max_len: None,
            todotxt_filepath: todo_path.clone(),
            done_filepath: done_path.clone(),
            env: PolybarModuleEnv {
                public_screen_filepath: dir.path().join("public_screen"),
                ..PolybarModuleEnv::new()
            },
        };
        let tasks = TodoFile::new(&todo_path, &done_path)
            .unwrap()
            .load_tasks()
            .unwrap();

        assert_eq!(
            module.try_update().unwrap(),
            TodoTxtModuleState::Active {
                pending_count: 2,
                next_task: tasks.get(1).cloned(),
                last_fs_change: Some(metadata(&todo_path).unwrap().modified().unwrap()),
            }
        );
    }

    #[test]
    fn test_render() {
        // SAFETY: actually NOT safe, but this is for tests only, and we are feeling lucky
        unsafe {
            env::set_var("TODO_FILE", "/dev/null");
            env::set_var("DONE_FILE", "/dev/null");
        }
        let xdg_dirs = xdg::BaseDirectories::new();
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
                "%{{F#f1e9d2}}%{{F-}} %{{A1:touch {}/public_screen:}}10 😌%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            next_task: Some(Task {
                priority: None,
                text: "todo".to_owned(),
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#f1e9d2}}%{{F-}} %{{A1:touch {}/public_screen:}}10 todo%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            next_task: Some(Task {
                priority: Some('D'),
                text: "todo".to_owned(),
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#f1e9d2}}%{{F-}} %{{A1:touch {}/public_screen:}}10 todo%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            next_task: Some(Task {
                priority: Some('D'),
                text: "todo".to_owned(),
                attributes: vec![("due".to_owned(), ZonedDateTime::now().date().to_string())],
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#f1e9d2}}%{{F-}} %{{A1:touch {}/public_screen:}}10 %{{u#d56500}}%{{+u}}todo%{{-u}}%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            next_task: Some(Task {
                priority: Some('B'),
                text: "todo".to_owned(),
                attributes: vec![("due".to_owned(), "2000-01-01".to_owned())],
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#f1e9d2}}%{{F-}} %{{A1:touch {}/public_screen:}}10 %{{u#d56500}}%{{+u}}todo%{{-u}}%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            next_task: Some(Task {
                priority: Some('B'),
                text: "todo".to_owned(),
                attributes: vec![("due".to_owned(), "2999-01-01".to_owned())],
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#f1e9d2}}%{{F-}} %{{A1:touch {}/public_screen:}}10 %{{u#ac8300}}%{{+u}}todo%{{-u}}%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            next_task: Some(Task {
                priority: Some('C'),
                text: "todo".to_owned(),
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#f1e9d2}}%{{F-}} %{{A1:touch {}/public_screen:}}10 %{{u#8faaab}}%{{+u}}todo%{{-u}}%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            next_task: Some(Task {
                priority: Some('A'),
                text: "todo".to_owned(),
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#f1e9d2}}%{{F-}} %{{A1:touch {}/public_screen:}}10 %{{u#d56500}}%{{+u}}todo%{{-u}}%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let module = TodoTxtModule::new(Some(7)).unwrap();

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            next_task: Some(Task {
                priority: None,
                text: "todo".to_owned(),
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#f1e9d2}}%{{F-}} %{{A1:touch {}/public_screen:}}10 todo%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 101,
            next_task: Some(Task {
                priority: None,
                text: "todo".to_owned(),
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#f1e9d2}}%{{F-}} %{{A1:touch {}/public_screen:}}101 to…%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 1011,
            next_task: Some(Task {
                priority: None,
                text: "todo".to_owned(),
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#f1e9d2}}%{{F-}} %{{A1:touch {}/public_screen:}}1011 t…%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Active {
            pending_count: 10,
            next_task: Some(Task {
                priority: None,
                text: "todozzz".to_owned(),
                ..Task::default()
            }),
            last_fs_change: None,
        });
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#f1e9d2}}%{{F-}} %{{A1:touch {}/public_screen:}}10 tod…%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );

        let state = Some(TodoTxtModuleState::Paused);
        assert_eq!(
            module.render(&state),
            format!(
                "%{{F#f1e9d2}}%{{F-}} %{{A1:rm {}/public_screen:}}󰜺%{{A}}",
                runtime_dir.to_str().unwrap()
            )
        );
    }
}
