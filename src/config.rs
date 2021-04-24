use structopt::StructOpt;

#[derive(Clone, Debug, StructOpt)]
#[allow(non_camel_case_types)]
pub enum PolybarModuleName {
    #[structopt(about = "Start screen autolock module")]
    autolock,
    #[structopt(about = "Start mouse battery module")]
    battery_mouse,
    #[structopt(about = "Start Nvidia GPU module")]
    gpu_nvidia,
    #[structopt(about = "Start low bandwidth button module")]
    internet_bandwidth,
    #[structopt(about = "Start market trend module")]
    market,
    #[structopt(about = "Start PulseAudio module")]
    pulseaudio,
    #[structopt(about = "Start weather module")]
    wttr { location: Option<String> },
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
    pub market: Option<MarketModuleConfig>,
}

#[derive(Debug, serde::Deserialize)]
pub struct MarketModuleConfig {
    pub api_token: String,
}

pub fn parse_config_file() -> anyhow::Result<Config> {
    let binary_name = env!("CARGO_PKG_NAME");
    let xdg_dirs = xdg::BaseDirectories::with_prefix(binary_name)?;
    let config_filepath = xdg_dirs
        .find_config_file("config.toml")
        .ok_or_else(|| anyhow::anyhow!("Unable to find config file"))?;
    log::debug!("Config filepath: {:?}", config_filepath);

    let toml_data = std::fs::read_to_string(config_filepath)?;
    log::trace!("Config data: {:?}", toml_data);

    let config = toml::from_str(&toml_data)?;
    log::trace!("Config: {:?}", config);
    Ok(config)
}
