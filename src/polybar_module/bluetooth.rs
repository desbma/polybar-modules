use std::collections::HashMap;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::str::FromStr;

use lazy_static::lazy_static;

use crate::markup;
use crate::polybar_module::RenderablePolybarModule;
use crate::theme;

pub struct BluetoothModule {
    controller: BluetoothController,
    devices: HashMap<macaddr::MacAddr6, BluetoothDevice>,
    bluetoothctl_child: Child,
}

#[derive(Clone, Debug, PartialEq)]
struct BluetoothDevice {
    connected: bool,
    name: String,
}

struct BluetoothController {
    powered: bool,
    addr: macaddr::MacAddr6,
}

#[derive(Debug, PartialEq)]
pub struct BluetoothModuleState {
    controller_powered: bool,
    devices: Vec<BluetoothDevice>,
}

impl BluetoothModule {
    pub fn new(device_whitelist_addrs: Vec<macaddr::MacAddr6>) -> anyhow::Result<BluetoothModule> {
        let bluetoothctl_child = Command::new("bluetoothctl")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        Ok(BluetoothModule {
            controller: Self::probe_controller()?,
            devices: Self::probe_devices(&device_whitelist_addrs)?,
            bluetoothctl_child,
        })
    }

    fn try_update(&mut self) -> anyhow::Result<BluetoothModuleState> {
        Ok(BluetoothModuleState {
            controller_powered: self.controller.powered,
            devices: self.devices.values().cloned().collect(),
        })
    }

    fn bluetoothcl_cmd(args: &[&str]) -> anyhow::Result<String> {
        let output = Command::new("bluetoothctl")
            .args(args)
            .stderr(Stdio::null())
            .output()?;
        if !output.status.success() {
            anyhow::bail!("bluetoothctl invocation failed");
        }

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
            let name = device_match.get(3).unwrap().as_str().to_string();
            let connected = Self::bluetoothcl_cmd(&["info", addr_str])?
                .lines()
                .filter_map(|l| CONNECTED_DEVICE_REGEX.captures(l))
                .map(|c| c.get(1).unwrap().as_str())
                .next()
                .ok_or_else(|| anyhow::anyhow!("Unable to probe device connected state"))?
                == "yes";
            let device = BluetoothDevice { connected, name };

            log::debug!("New known device ({}): {:?}", addr, device);
            devices.insert(addr, device);
        }

        Ok(devices)
    }
}

impl Drop for BluetoothModule {
    #[allow(unused_must_use)]
    fn drop(&mut self) {
        self.bluetoothctl_child.kill();
    }
}

impl RenderablePolybarModule for BluetoothModule {
    type State = Option<BluetoothModuleState>;

    fn wait_update(&mut self, prev_state: &Option<Self::State>) {
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
                let read_buf = &strip_ansi_escapes::strip(&buffer[0..read_count]).unwrap();
                let read_str = String::from_utf8_lossy(read_buf);
                log::trace!("{} bytes read: {:?}", read_count, read_str);

                // Parse event lines
                for line in read_str.lines() {
                    lazy_static! {
                        static ref POWER_EVENT_REGEX: regex::Regex =
                            regex::Regex::new("^\\[CHG\\] Controller (([A-F0-9]{2}:){5}[A-F0-9]{2}) Powered: (yes|no)$").unwrap();
                        static ref CONNECT_EVENT_REGEX: regex::Regex =
                            regex::Regex::new("^\\[CHG\\] Device (([A-F0-9]{2}:){5}[A-F0-9]{2}) Connected: (yes|no)$").unwrap();
                        static ref AUTHORIZE_EVENT_REGEX: regex::Regex =
                            regex::Regex::new("^\\[agent\\] Authorize service [0-9a-f-]+ \\(yes/no\\): $").unwrap();
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

                        if addr != self.controller.addr {
                            log::warn!("Power event for unknown controller");
                        } else {
                            self.controller.powered = status;
                            need_render = true;
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

                        need_render = true;
                    } else if let Some(authorize_event_match) = AUTHORIZE_EVENT_REGEX.captures(line)
                    {
                        log::trace!("{:?}", authorize_event_match);

                        need_render = true;
                    } else {
                        log::debug!("Ignored line: {:?}", line);
                    }
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
                let mut fragments: Vec<String> = vec![format!(
                    "{} {}",
                    markup::style("", Some(theme::Color::MainIcon), None, None, None),
                    if state.controller_powered {
                        ""
                    } else {
                        ""
                    },
                )];
                for device in &state.devices {
                    fragments.push(markup::style(
                        &format!(
                            "{}{}",
                            if device.connected { "" } else { "" },
                            device.name
                        ),
                        None,
                        if device.connected {
                            Some(theme::Color::Foreground)
                        } else {
                            None
                        },
                        None,
                        None,
                    ));
                }
                fragments.join(" ")
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
        let module = BluetoothModule::new(vec![]).unwrap();

        let state = Some(BluetoothModuleState {
            controller_powered: false,
            devices: vec![],
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} ");

        let state = Some(BluetoothModuleState {
            controller_powered: true,
            devices: vec![],
        });
        assert_eq!(module.render(&state), "%{F#eee8d5}%{F-} ");

        let state = Some(BluetoothModuleState {
            controller_powered: true,
            devices: vec![
                BluetoothDevice {
                    connected: false,
                    name: "D1".to_string(),
                },
                BluetoothDevice {
                    connected: true,
                    name: "D2".to_string(),
                },
            ],
        });
        assert_eq!(
            module.render(&state),
            "%{F#eee8d5}%{F-}  D1 %{u#93a1a1}%{+u}D2%{-u}"
        );

        let state = None;
        assert_eq!(module.render(&state), "%{F#cb4b16}%{F-}");
    }
}
