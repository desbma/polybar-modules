#![feature(exit_status_error)]
#![feature(hash_extract_if)]
use std::io::{self, IsTerminal};

use anyhow::Context;
use config::PolybarModuleName;
use structopt::StructOpt;

mod config;
mod markup;
mod polybar_module;
mod theme;

#[allow(clippy::too_many_lines)]
fn main() -> anyhow::Result<()> {
    // Init logger
    if io::stdout().is_terminal() {
        simple_logger::SimpleLogger::new()
            .with_level(if cfg!(debug_assertions) {
                log::LevelFilter::Debug
            } else {
                log::LevelFilter::Info
            })
            .env()
            .init()
            .context("Failed to setup logger")?;
    }

    // Parse command line args
    let cl_opts = config::CommandLineOpts::from_args();
    log::trace!("{:?}", cl_opts);

    // Parse config file
    let cfg = config::parse_config_file();

    // Init stuff
    let module: polybar_module::PolybarModule = match cl_opts.module {
        PolybarModuleName::arch_updates => polybar_module::PolybarModule::ArchUpdates(
            polybar_module::arch_updates::ArchUpdatesModule::new()
                .context("Failed to initialize Arch updates module")?,
        ),
        PolybarModuleName::autolock => polybar_module::PolybarModule::Autolock(
            polybar_module::autolock::AutolockModule::new()
                .context("Failed to initialize autolock module")?,
        ),
        PolybarModuleName::battery_mouse => polybar_module::PolybarModule::BatteryMouse(
            polybar_module::battery_mouse::BatteryMouseModule::new(),
        ),
        PolybarModuleName::bluetooth {
            device_whitelist_addrs,
        } => polybar_module::PolybarModule::Bluetooth(
            polybar_module::bluetooth::BluetoothModule::new(&device_whitelist_addrs)
                .context("Failed to initialize bluetooth module")?,
        ),
        PolybarModuleName::cpu_freq => polybar_module::PolybarModule::CpuFreq(
            polybar_module::cpu_freq::CpuFreqModule::new()
                .context("Failed to initialize CPU frequency module")?,
        ),
        PolybarModuleName::cpu_top { max_len } => polybar_module::PolybarModule::CpuTop(
            polybar_module::cpu_top::CpuTopModule::new(max_len),
        ),
        PolybarModuleName::debian_updates => polybar_module::PolybarModule::DebianUpdates(
            polybar_module::debian_updates::DebianUpdatesModule::new()
                .context("Failed to initialize Debian updates module")?,
        ),
        PolybarModuleName::gpu_nvidia => polybar_module::PolybarModule::GpuNvidia(
            polybar_module::gpu_nvidia::GpuNvidiaModule::new()
                .context("Failed to initialize Nvidia GPU module")?,
        ),
        PolybarModuleName::home_power => {
            let home_power_cfg = cfg
                .and_then(|c| {
                    c.module
                        .ok_or_else(|| anyhow::anyhow!("Missing 'module' config section"))
                })
                .and_then(|c| {
                    c.home_power
                        .ok_or_else(|| anyhow::anyhow!("Missing 'home_power' config section"))
                })
                .context("Unable to get home power module config from config file")?;
            polybar_module::PolybarModule::HomePower(
                polybar_module::home_power::HomePowerModule::new(&home_power_cfg)?,
            )
        }
        PolybarModuleName::internet_bandwidth => polybar_module::PolybarModule::InternetBandwidth(
            polybar_module::internet_bandwidth::InternetBandwidthModule::new(),
        ),
        PolybarModuleName::market => {
            polybar_module::PolybarModule::Market(polybar_module::market::MarketModule::new()?)
        }
        PolybarModuleName::network_status => {
            let network_status_cfg = cfg
                .and_then(|c| {
                    c.module
                        .ok_or_else(|| anyhow::anyhow!("Missing 'module' config section"))
                })
                .and_then(|c| {
                    c.network_status
                        .ok_or_else(|| anyhow::anyhow!("Missing 'network_status' config section"))
                })
                .context("Unable to get network status module config from config file")?;
            polybar_module::PolybarModule::NetworkStatus(
                polybar_module::network_status::NetworkStatusModule::new(network_status_cfg)
                    .context("Failed to initialize network status module")?,
            )
        }
        PolybarModuleName::player { max_len } => polybar_module::PolybarModule::Player(
            polybar_module::player::PlayerModule::new(max_len)
                .context("Failed to initialize player module")?,
        ),
        PolybarModuleName::progressbar_server { max_len } => {
            polybar_module::PolybarModule::ProgressBarServer(
                polybar_module::progressbar_server::ProgressBarServerModule::new(max_len)
                    .context("Failed to initialize progress bar server module")?,
            )
        }
        PolybarModuleName::pulseaudio => polybar_module::PolybarModule::PulseAudio(
            polybar_module::pulseaudio::PulseAudioModule::new()
                .context("Failed to initialize Pulseaudio module")?,
        ),
        PolybarModuleName::syncthing => {
            let xdg_dirs = xdg::BaseDirectories::with_prefix("syncthing")
                .context("Unable fo find Synthing config directory")?;
            let st_config_filepath = xdg_dirs
                .find_config_file("config.xml")
                .context("Unable fo find Synthing config file")?;
            polybar_module::PolybarModule::Syncthing(
                polybar_module::syncthing::SyncthingModule::new(&st_config_filepath)
                    .context("Failed to initialize Syncthing module")?,
            )
        }
        PolybarModuleName::todotxt { max_len } => polybar_module::PolybarModule::TodoTxt(
            polybar_module::todotxt::TodoTxtModule::new(max_len)
                .context("Failed to initialize Todo.txt module")?,
        ),
        PolybarModuleName::wttr { location } => polybar_module::PolybarModule::Wttr(
            polybar_module::wttr::WttrModule::new(location.as_ref())
                .context("Failed to initialize Wttr module")?,
        ),
        PolybarModuleName::xmonad => polybar_module::PolybarModule::Xmonad(
            polybar_module::xmonad::XmonadModule::new()
                .context("Failed to initialize Xmonad module")?,
        ),
    };

    // Update/render loop, dynamic dispatch sadness, sadly https://crates.io/crates/enum_dispatch does not work here
    match module {
        polybar_module::PolybarModule::ArchUpdates(module) => render_loop(module),
        polybar_module::PolybarModule::Autolock(module) => render_loop(module),
        polybar_module::PolybarModule::BatteryMouse(module) => render_loop(module),
        polybar_module::PolybarModule::Bluetooth(module) => render_loop(module),
        polybar_module::PolybarModule::CpuFreq(module) => render_loop(module),
        polybar_module::PolybarModule::CpuTop(module) => render_loop(module),
        polybar_module::PolybarModule::DebianUpdates(module) => render_loop(module),
        polybar_module::PolybarModule::GpuNvidia(module) => render_loop(module),
        polybar_module::PolybarModule::HomePower(module) => render_loop(module),
        polybar_module::PolybarModule::InternetBandwidth(module) => render_loop(module),
        polybar_module::PolybarModule::Market(module) => render_loop(module),
        polybar_module::PolybarModule::NetworkStatus(module) => render_loop(module),
        polybar_module::PolybarModule::Player(module) => render_loop(module),
        polybar_module::PolybarModule::ProgressBarServer(module) => render_loop(module),
        polybar_module::PolybarModule::PulseAudio(module) => render_loop(module),
        polybar_module::PolybarModule::Syncthing(module) => render_loop(module),
        polybar_module::PolybarModule::TodoTxt(module) => render_loop(module),
        polybar_module::PolybarModule::Wttr(module) => render_loop(module),
        polybar_module::PolybarModule::Xmonad(module) => render_loop(module),
    };
}

fn render_loop<T>(mut module: T) -> !
where
    T: polybar_module::RenderablePolybarModule,
{
    let mut prev_state: Option<T::State> = None;
    loop {
        // Update
        module.wait_update(prev_state.as_ref());
        let state = module.update();
        log::debug!("{:?}", state);

        // Render or skip?
        let do_render = match &prev_state {
            Some(prev_state) => prev_state != &state,
            None => true,
        };
        if !do_render {
            continue;
        }

        // Render
        let output = module.render(&state);
        println!("{output}");
        prev_state = Some(state);
    }
}
