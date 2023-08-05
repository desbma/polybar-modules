use std::fs::read_to_string;

use structopt::StructOpt;

#[derive(Clone, Debug, StructOpt)]
#[allow(non_camel_case_types)]
pub enum PolybarModuleName {
    #[structopt(about = "Start Arch Linux update module")]
    arch_updates,
    #[structopt(about = "Start screen autolock module")]
    autolock,
    #[structopt(about = "Start mouse battery module")]
    battery_mouse,
    #[structopt(about = "Start bluetooth module")]
    bluetooth {
        device_whitelist_addrs: Vec<macaddr::MacAddr6>,
    },
    #[structopt(about = "Start CPU frequency module")]
    cpu_freq,
    #[structopt(about = "Start Debian update module")]
    debian_updates,
    #[structopt(about = "Start Nvidia GPU module")]
    gpu_nvidia,
    #[structopt(about = "Start low bandwidth button module")]
    internet_bandwidth,
    #[structopt(about = "Start market trend module")]
    market,
    #[structopt(about = "Start network status module")]
    network_status,
    #[structopt(about = "Start progress bar server module")]
    progressbar_server { max_len: usize },
    #[structopt(about = "Start PulseAudio module")]
    pulseaudio,
    #[structopt(about = "Start Syncthing module")]
    syncthing,
    #[structopt(about = "Start Taskwarrior module")]
    taskwarrior { max_len: Option<usize> },
    #[structopt(about = "Start Todo.txt module")]
    todotxt { max_len: Option<usize> },
    #[structopt(about = "Start weather module")]
    wttr { location: Option<String> },
    #[structopt(about = "Start Xmonad module")]
    xmonad,
}

#[derive(Debug, StructOpt)]
#[structopt(version=env!("CARGO_PKG_VERSION"), about="Polybar modules.")]
pub struct CommandLineOpts {
    #[structopt(subcommand, about = "Polybar module to start")]
    pub module: PolybarModuleName,
}

#[derive(Debug, serde::Deserialize)]
pub struct Config {
    pub module: Option<ModuleConfig>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ModuleConfig {
    pub network_status: Option<NetworkStatusModuleConfig>,
}

#[derive(Debug, serde::Deserialize)]
pub struct NetworkStatusHost {
    pub name: String,
    pub host: String,
    #[serde(default)]
    pub warn_unreachable: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct NetworkStatusModuleConfig {
    pub hosts: Vec<NetworkStatusHost>,
}

pub fn parse_config_file() -> anyhow::Result<Config> {
    let binary_name = env!("CARGO_PKG_NAME");
    let xdg_dirs = xdg::BaseDirectories::with_prefix(binary_name)?;
    let config_filepath = xdg_dirs
        .find_config_file("config.toml")
        .ok_or_else(|| anyhow::anyhow!("Unable to find config file"))?;
    log::debug!("Config filepath: {:?}", config_filepath);

    let toml_data = read_to_string(config_filepath)?;
    log::trace!("Config data: {:?}", toml_data);

    let config = toml::from_str(&toml_data)?;
    log::trace!("Config: {:?}", config);
    Ok(config)
}
