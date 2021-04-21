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
