use std::fs::read_to_string;

#[derive(Clone, Debug, clap::Parser)]
#[expect(non_camel_case_types, clippy::doc_markdown)]
pub(crate) enum PolybarModuleName {
    /// Start Arch Linux update module
    arch_updates,
    /// Start screen autolock module
    autolock,
    /// Start mouse battery module
    battery_mouse,
    /// Start bluetooth module
    bluetooth {
        device_whitelist_addrs: Vec<macaddr::MacAddr6>,
    },
    /// Start CPU frequency module
    cpu_freq,
    /// Start CPU top process module
    cpu_top { max_len: Option<usize> },
    /// Start Debian update module
    debian_updates,
    /// Start Nvidia GPU module
    gpu_nvidia,
    /// Start home power module
    home_power,
    /// Start low bandwidth button module
    internet_bandwidth,
    /// Start market trend module
    market,
    /// Start network status module
    network_status,
    /// Start notifications status module
    notifications,
    /// Start player status module
    player { max_len: usize },
    /// Start progress bar server module
    progressbar_server { max_len: usize },
    /// Start PulseAudio module
    pulseaudio,
    /// Start Syncthing module
    syncthing,
    /// Start Todo.txt module
    todotxt { max_len: Option<usize> },
    /// Start weather module
    wttr { location: Option<String> },
    /// Start Xmonad module
    xmonad,
}

#[derive(Debug, clap::Parser)]
#[command(version, about = "Polybar modules.")]
pub(crate) struct CommandLineOpts {
    /// Polybar module to start
    #[command(subcommand)]
    pub module: PolybarModuleName,
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct Config {
    pub module: Option<ModuleConfig>,
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct ModuleConfig {
    pub home_power: Option<HomePowerModuleConfig>,
    pub network_status: Option<NetworkStatusModuleConfig>,
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct HomePowerModuleConfig {
    pub inverter_modbus: InverterModbusConfig,
    pub shelly_devices: Vec<ShellyDeviceConfig>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct InverterModbusConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct ShellyDeviceConfig {
    pub name: String,
    pub host: String,
    pub password: String,
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct NetworkStatusHost {
    pub name: String,
    pub host: String,
    #[serde(default)]
    pub warn_unreachable: bool,
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct NetworkStatusModuleConfig {
    pub hosts: Vec<NetworkStatusHost>,
}

pub(crate) fn parse_config_file() -> anyhow::Result<Config> {
    let binary_name = env!("CARGO_PKG_NAME");
    let xdg_dirs = xdg::BaseDirectories::with_prefix(binary_name);
    let config_filepath = xdg_dirs
        .find_config_file("config.toml")
        .ok_or_else(|| anyhow::anyhow!("Unable to find config file"))?;
    log::debug!("Config filepath: {config_filepath:?}");

    let toml_data = read_to_string(config_filepath)?;
    log::trace!("Config data: {toml_data:?}");

    let config = toml::from_str(&toml_data)?;
    log::trace!("Config: {config:?}");
    Ok(config)
}
