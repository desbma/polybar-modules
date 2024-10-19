use std::{
    collections::HashMap,
    io::Read,
    process::{Child, Command, Stdio},
    str::FromStr,
};

use anyhow::Context;
use lazy_static::lazy_static;

use crate::{markup, polybar_module::RenderablePolybarModule, theme};

pub(crate) struct BluetoothModule {
    controller: BluetoothController,
    devices: HashMap<macaddr::MacAddr6, BluetoothDevice>,
    bluetoothctl_child: Child,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BluetoothDevice {
    connected: bool,
    name: String,
    addr: macaddr::MacAddr6,
}

struct BluetoothController {
    powered: bool,
    addr: macaddr::MacAddr6,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct BluetoothModuleState {
    controller_powered: bool,
    devices: Vec<BluetoothDevice>,
}

impl BluetoothModule {
    pub(crate) fn new(device_whitelist_addrs: &[macaddr::MacAddr6]) -> anyhow::Result<Self> {
        let bluetoothctl_child = Command::new("bluetoothctl")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        Ok(Self {
            controller: Self::probe_controller()?,
            devices: Self::probe_devices(device_whitelist_addrs)?,
            bluetoothctl_child,
        })
    }

    fn bluetoothcl_cmd(args: &[&str]) -> anyhow::Result<String> {
        let output = Command::new("bluetoothctl")
            .args(args)
            .stderr(Stdio::null())
            .output()?;
        output
            .status
            .exit_ok()
            .context("bluetoothctl exited with error")?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn probe_controller() -> anyhow::Result<BluetoothController> {
        let show_output = Self::bluetoothcl_cmd(&["show"])?;
        lazy_static! {
            static ref CONTROLER_POWERED_REGEX: regex::Regex =
                regex::Regex::new("^\tPowered: (yes|no)$").unwrap();
            static ref CONTROLER_HEADER_REGEX: regex::Regex =
                regex::Regex::new("^Controller (([A-F0-9]{2}:){5}[A-F0-9]{2}) ").unwrap();
        }
        // TODO warn if more than one controller found
        let powered = show_output
            .lines()
            .filter_map(|l| CONTROLER_POWERED_REGEX.captures(l))
            .map(|c| c.get(1).unwrap().as_str())
            .next()
            .ok_or_else(|| anyhow::anyhow!("Unable to probe controller powered state"))?
            == "yes";
        let addr = show_output
            .lines()
            .filter_map(|l| CONTROLER_HEADER_REGEX.captures(l))
            .map(|c| macaddr::MacAddr6::from_str(c.get(1).unwrap().as_str()))
            .next()
            .ok_or_else(|| anyhow::anyhow!("Unable to probe controller address"))??;

        log::debug!(
            "Controler {} powered: {}",
            addr,
            if powered { "YES" } else { "NO" },
        );
        Ok(BluetoothController { powered, addr })
    }

    fn probe_devices(
        whitelist_addrs: &[macaddr::MacAddr6],
    ) -> anyhow::Result<HashMap<macaddr::MacAddr6, BluetoothDevice>> {
        let mut devices: HashMap<macaddr::MacAddr6, BluetoothDevice> = HashMap::new();

        lazy_static! {
            static ref KNOWN_DEVICE_REGEX: regex::Regex =
                regex::Regex::new("^Device (([A-F0-9]{2}:){5}[A-F0-9]{2}) (.*)$").unwrap();
            static ref CONNECTED_DEVICE_REGEX: regex::Regex =
                regex::Regex::new("^\tConnected: (yes|no)$").unwrap();
        }
        for device_match in Self::bluetoothcl_cmd(&["devices"])?
            .lines()
            .filter_map(|l| KNOWN_DEVICE_REGEX.captures(l))
        {
            let addr_str = device_match.get(1).unwrap().as_str();
            let addr = macaddr::MacAddr6::from_str(addr_str)?;
            if !whitelist_addrs.is_empty() && !whitelist_addrs.contains(&addr) {
                log::warn!(
                    "Ignoring device {} not in whitelist {:?}",
                    addr,
                    whitelist_addrs
                );
                continue;
            }
            let name = device_match.get(3).unwrap().as_str().to_owned();
            let connected = Self::bluetoothcl_cmd(&["info", addr_str])?
                .lines()
                .filter_map(|l| CONNECTED_DEVICE_REGEX.captures(l))
                .map(|c| c.get(1).unwrap().as_str())
                .next()
                .ok_or_else(|| anyhow::anyhow!("Unable to probe device connected state"))?
                == "yes";
            let device = BluetoothDevice {
                connected,
                name,
                addr,
            };

            log::debug!("New known device ({}): {:?}", addr, device);
            devices.insert(addr, device);
        }

        Ok(devices)
    }
}

impl Drop for BluetoothModule {
    #[expect(unused_must_use)]
    fn drop(&mut self) {
        self.bluetoothctl_child.kill();
    }
}

impl RenderablePolybarModule for BluetoothModule {
    type State = BluetoothModuleState;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        if prev_state.is_some() {
            let mut buffer = [0; 65536];
            let mut need_render = false;
            while !need_render {
                // Read new data
                let read_count = self
                    .bluetoothctl_child
                    .stdout
                    .as_mut()
                    .unwrap()
                    .read(&mut buffer)
                    .unwrap();
                let read_buf = &strip_ansi_escapes::strip(&buffer[0..read_count]);
                let read_str = String::from_utf8_lossy(read_buf);
                log::trace!("{} bytes read: {:?}", read_count, read_str);

                // Parse event lines
                for line in read_str.lines() {
                    lazy_static! {
                        static ref POWER_EVENT_REGEX: regex::Regex =
                            regex::Regex::new("\\[CHG\\] Controller (([A-F0-9]{2}:){5}[A-F0-9]{2}) Powered: (yes|no)$").unwrap();
                        static ref CONNECT_EVENT_REGEX: regex::Regex =
                            regex::Regex::new("\\[CHG\\] Device (([A-F0-9]{2}:){5}[A-F0-9]{2}) Connected: (yes|no)$").unwrap();
                    }

                    if let Some(power_event_match) = POWER_EVENT_REGEX.captures(line) {
                        log::trace!("{:?}", power_event_match);

                        let addr: macaddr::MacAddr6 =
                            macaddr::MacAddr6::from_str(power_event_match.get(1).unwrap().as_str())
                                .unwrap();
                        let status = power_event_match.get(3).unwrap().as_str() == "yes";

                        log::debug!(
                            "Controller {} powered {}",
                            addr,
                            if status { "ON" } else { "OFF" }
                        );

                        if addr == self.controller.addr {
                            self.controller.powered = status;
                            if !status {
                                self.devices.values_mut().for_each(|d| d.connected = false);
                            }
                            need_render = true;
                        } else {
                            log::warn!("Power event for unknown controller");
                        }
                    } else if let Some(connect_event_match) = CONNECT_EVENT_REGEX.captures(line) {
                        log::trace!("{:?}", connect_event_match);

                        let addr: macaddr::MacAddr6 = macaddr::MacAddr6::from_str(
                            connect_event_match.get(1).unwrap().as_str(),
                        )
                        .unwrap();
                        let status = connect_event_match.get(3).unwrap().as_str() == "yes";

                        log::debug!(
                            "Device {} {}connected",
                            addr,
                            if status { "" } else { "dis" }
                        );

                        if let Some(d) = self.devices.get_mut(&addr) {
                            d.connected = status;
                            need_render = true;
                        } else {
                            log::warn!("Ignoring event for unknown device {}", addr);
                        }
                    } else {
                        log::debug!("Ignored line: {:?}", line);
                    }
                }
            }
        }
    }

    fn update(&mut self) -> Self::State {
        let mut devices = if self.controller.powered {
            self.devices.values().cloned().collect()
        } else {
            vec![]
        };
        devices.sort_by_key(|d| d.name.clone());
        BluetoothModuleState {
            controller_powered: self.controller.powered,
            devices,
        }
    }

    fn render(&self, state: &Self::State) -> String {
        let mut fragments: Vec<String> = vec![format!(
            "{} {}",
            markup::style("", Some(theme::Color::MainIcon), None, None, None),
            if state.controller_powered {
                markup::action(
                    "",
                    markup::PolybarAction {
                        type_: markup::PolybarActionType::ClickLeft,
                        command: "bluetoothctl power off".to_owned(),
                    },
                )
            } else {
                markup::action(
                    "",
                    markup::PolybarAction {
                        type_: markup::PolybarActionType::ClickLeft,
                        command: "bluetoothctl power on".to_owned(),
                    },
                )
            },
        )];
        for device in &state.devices {
            let name = theme::ellipsis(&theme::shorten_model_name(&device.name), Some(4));
            let device_markup = markup::style(
                &name,
                None,
                device.connected.then_some(theme::Color::Foreground),
                None,
                None,
            );
            let action_markup = markup::action(
                &device_markup,
                markup::PolybarAction {
                    type_: markup::PolybarActionType::ClickLeft,
                    command: format!(
                        "bluetoothctl {}connect {}",
                        if device.connected { "dis" } else { "" },
                        device.addr
                    ),
                },
            );
            fragments.push(action_markup);
        }
        fragments.join(" ")
    }
}

#[cfg(test)]
#[expect(clippy::shadow_unrelated)]
mod tests {
    use std::{
        env,
        fs::{File, Permissions},
        io::Write,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
    };

    use super::*;

    fn update_path(dir: &str) -> std::ffi::OsString {
        let path_orig = env::var_os("PATH").unwrap();

        let mut paths_vec = env::split_paths(&path_orig).collect::<Vec<_>>();
        paths_vec.insert(0, PathBuf::from(dir));

        let paths = env::join_paths(paths_vec).unwrap();
        env::set_var("PATH", paths);

        path_orig
    }

    #[test]
    fn test_render() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let fake_bluetoothctl_filepath = tmp_dir.path().join("bluetoothctl");
        let mut fake_bluetoothctl_file = File::create(fake_bluetoothctl_filepath).unwrap();
        write!(
            &mut fake_bluetoothctl_file,
            concat!(
                "#!/bin/sh\n",
                "if [ $1 = 'show' ]; then\n",
                "  echo 'Controller 01:02:03:04:05:06 '\n",
                "  echo '\tPowered: yes'\n",
                "elif [ $# -eq 0 ]; then\n",
                "  exec sleep inf\n",
                "fi\n"
            )
        )
        .unwrap();
        fake_bluetoothctl_file
            .set_permissions(Permissions::from_mode(0o700))
            .unwrap();
        drop(fake_bluetoothctl_file);
        let path_orig = update_path(tmp_dir.path().to_str().unwrap());

        let module = BluetoothModule::new(&[]).unwrap();

        let state = BluetoothModuleState {
            controller_powered: false,
            devices: vec![],
        };
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} %{A1:bluetoothctl power on:}\u{f204}%{A}"
        );

        let state = BluetoothModuleState {
            controller_powered: true,
            devices: vec![],
        };
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} %{A1:bluetoothctl power off:}\u{f205}%{A}"
        );

        let state = BluetoothModuleState {
            controller_powered: true,
            devices: vec![
                BluetoothDevice {
                    connected: false,
                    name: "D1".to_owned(),
                    addr: macaddr::MacAddr6::from_str("01:02:03:04:05:06").unwrap(),
                },
                BluetoothDevice {
                    connected: true,
                    name: "D2".to_owned(),
                    addr: macaddr::MacAddr6::from_str("02:01:03:04:05:06").unwrap(),
                },
            ],
        };
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-} %{A1:bluetoothctl power off:}\u{f205}%{A} %{A1:bluetoothctl connect 01\\:02\\:03\\:04\\:05\\:06:}D1%{A} %{A1:bluetoothctl disconnect 02\\:01\\:03\\:04\\:05\\:06:}%{u#93a1a1}%{+u}D2%{-u}%{A}"
        );

        env::set_var("PATH", path_orig);
    }
}
