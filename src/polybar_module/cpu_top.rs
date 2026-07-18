use std::{
    ffi::{OsStr, OsString},
    path::Path,
    thread::sleep,
    time::Duration,
};

use sysinfo::{
    MINIMUM_CPU_UPDATE_INTERVAL, ProcessRefreshKind, ProcessesToUpdate, System, ThreadKind,
    UpdateKind, get_current_pid,
};

use crate::{
    markup,
    polybar_module::RenderablePolybarModule,
    theme::{self, ICON_WARNING},
};

pub(crate) struct CpuTopModule {
    max_len: Option<usize>,
    system: System,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct CpuTopModuleState {
    cpu_prct: u32,
    process_name: String,
}

impl CpuTopModule {
    pub(crate) fn new(max_len: Option<usize>) -> Self {
        Self {
            max_len,
            system: System::new(),
        }
    }

    fn refresh_processes(&mut self) {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .without_tasks()
                .with_cpu()
                .with_exe(UpdateKind::Always)
                .with_cmd(UpdateKind::Always),
        );
    }

    fn try_update(&mut self) -> anyhow::Result<CpuTopModuleState> {
        // Per-process CPU usage is a diff between the two most recent refreshes, so a priming
        // refresh is needed for the first update to report real values instead of 0% everywhere
        if self.system.processes().is_empty() {
            self.refresh_processes();
            sleep(MINIMUM_CPU_UPDATE_INTERVAL);
        }
        self.refresh_processes();

        // The sampler's own /proc scan makes it a CPU consumer; exclude it so an idle system
        // does not just report this process back
        let current_pid = get_current_pid().map_err(anyhow::Error::msg)?;
        let proc = self
            .system
            .processes()
            .values()
            .filter(|process| process.pid() != current_pid)
            .max_by(|a, b| a.cpu_usage().total_cmp(&b.cpu_usage()))
            .ok_or_else(|| anyhow::anyhow!("No process found"))?;

        let process_name =
            Self::resolve_process_name(proc.thread_kind(), proc.exe(), proc.cmd(), proc.name())
                .ok_or_else(|| anyhow::anyhow!("Unable to resolve process name"))?;
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let cpu_prct = proc.cpu_usage().round().clamp(0.0, 99.0) as u32;

        Ok(CpuTopModuleState {
            cpu_prct,
            process_name,
        })
    }

    fn resolve_process_name(
        thread_kind: Option<ThreadKind>,
        exe: Option<&Path>,
        cmd: &[OsString],
        name: &OsStr,
    ) -> Option<String> {
        if thread_kind == Some(ThreadKind::Kernel) {
            return Some("[kthread]".to_owned());
        }
        let cmd_name = cmd.first().and_then(|arg| Path::new(arg).file_name());
        exe.and_then(Path::file_name)
            .or(cmd_name)
            .or_else(|| (!name.is_empty()).then_some(name))
            .map(|n| n.to_string_lossy().into_owned())
    }
}

impl RenderablePolybarModule for CpuTopModule {
    type State = Option<CpuTopModuleState>;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        // TODO increase sleep delay if on battery?
        if let Some(prev_state) = prev_state {
            let sleep_duration = match prev_state {
                Some(state) if state.cpu_prct > 30 => Duration::from_secs(1),
                _ => Duration::from_secs(3),
            };
            sleep(sleep_duration);
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
        let max_proc_len = self.max_len.map(|v| v - 4);
        match state {
            Some(state) => {
                let mut markup = markup::Markup::new(format!(
                    "{: >2}% {}",
                    state.cpu_prct,
                    theme::pad(
                        &theme::ellipsis(&state.process_name, max_proc_len),
                        max_proc_len
                    ),
                ));
                if state.cpu_prct >= 90 {
                    markup = markup.fg(theme::Color::Attention);
                } else if state.cpu_prct >= 50 {
                    markup = markup.fg(theme::Color::Notice);
                }
                markup.into_string()
            }
            None => markup::Markup::new(ICON_WARNING)
                .fg(theme::Color::Attention)
                .into_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::ffi::OsStrExt as _;

    use super::*;

    #[test]
    fn test_resolve_process_name() {
        let resolve = |thread_kind, exe: Option<&str>, args: &[&str], name: &str| -> String {
            let cmd: Vec<OsString> = args.iter().copied().map(OsString::from).collect();
            CpuTopModule::resolve_process_name(
                thread_kind,
                exe.map(Path::new),
                &cmd,
                OsStr::new(name),
            )
            .unwrap()
        };

        // exe basename wins over the command and the name
        assert_eq!(
            resolve(
                None,
                Some("/usr/lib/firefox/firefox"),
                &["/usr/bin/from-command"],
                "from-name"
            ),
            "firefox"
        );
        // exe unavailable: fall back to the command basename over the name
        assert_eq!(
            resolve(
                None,
                None,
                &["/usr/bin/polybar-modules", "network-status"],
                "from-name"
            ),
            "polybar-modules"
        );
        // empty exe path is treated as unavailable
        assert_eq!(
            resolve(None, Some(""), &["/usr/lib/Xorg", ":0"], "from-name"),
            "Xorg"
        );
        // kernel thread: identified by thread kind, not by empty exe/cmd
        assert_eq!(
            resolve(Some(ThreadKind::Kernel), None, &[], "kworker/0:1"),
            "[kthread]"
        );
        // userland process with neither exe nor command line: fall back to the name
        assert_eq!(resolve(None, None, &[], "mystery"), "mystery");
        // no usable identity: unresolved
        assert_eq!(
            CpuTopModule::resolve_process_name(None, None, &[], OsStr::new("")),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_non_utf8_process_name() {
        assert_eq!(
            CpuTopModule::resolve_process_name(None, None, &[], OsStr::from_bytes(b"process-\xff")),
            Some("process-\u{fffd}".to_owned())
        );
    }

    #[test]
    fn test_render() {
        let module = CpuTopModule::new(Some(10));

        for (cpu_prct, process_name, expected) in [
            (1, "bz", " 1%     bz"),
            (1, "bzzzzzzzzzzzzzzzz", " 1% bzzzz…"),
            (50, "bz", "%{F#ac8300}50%     bz%{F-}"),
            (99, "bz", "%{F#d56500}99%     bz%{F-}"),
        ] {
            let state = Some(CpuTopModuleState {
                cpu_prct,
                process_name: process_name.to_owned(),
            });
            assert_eq!(module.render(&state), expected);
        }

        let state = None;
        assert_eq!(
            module.render(&state),
            format!("%{{F#d56500}}{ICON_WARNING}%{{F-}}")
        );
    }
}
