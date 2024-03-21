use std::{fmt::Debug, path::PathBuf, sync::mpsc::channel, time::Duration};

use backoff::{ExponentialBackoff, ExponentialBackoffBuilder};
use notify::Watcher;

pub mod arch_updates;
pub mod autolock;
pub mod battery_mouse;
pub mod bluetooth;
pub mod cpu_freq;
pub mod cpu_top;
pub mod debian_updates;
pub mod gpu_nvidia;
pub mod home_power;
pub mod internet_bandwidth;
pub mod market;
pub mod network_status;
pub mod progressbar_server;
pub mod pulseaudio;
pub mod syncthing;
mod syncthing_rest;
pub mod taskwarrior;
pub mod todotxt;
pub mod wttr;
pub mod xmonad;

pub enum PolybarModule {
    ArchUpdates(arch_updates::ArchUpdatesModule),
    Autolock(autolock::AutolockModule),
    BatteryMouse(battery_mouse::BatteryMouseModule),
    Bluetooth(bluetooth::BluetoothModule),
    CpuFreq(cpu_freq::CpuFreqModule),
    CpuTop(cpu_top::CpuTopModule),
    DebianUpdates(debian_updates::DebianUpdatesModule),
    GpuNvidia(gpu_nvidia::GpuNvidiaModule),
    HomePower(home_power::HomePowerModule),
    InternetBandwidth(internet_bandwidth::InternetBandwidthModule),
    Market(market::MarketModule),
    NetworkStatus(network_status::NetworkStatusModule),
    ProgressBarServer(progressbar_server::ProgressBarServerModule),
    PulseAudio(pulseaudio::PulseAudioModule),
    Syncthing(syncthing::SyncthingModule),
    Taskwarrior(taskwarrior::TaskwarriorModule),
    TodoTxt(todotxt::TodoTxtModule),
    Wttr(wttr::WttrModule),
    Xmonad(xmonad::XmonadModule),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NetworkMode {
    Unrestricted,
    LowBandwith,
}

const TCP_REMOTE_TIMEOUT: Duration = Duration::from_secs(20);
const TCP_LOCAL_TIMEOUT: Duration = Duration::from_secs(5);

pub trait RenderablePolybarModule {
    type State: Debug + PartialEq;

    fn wait_update(&mut self, prev_state: &Option<Self::State>);

    fn update(&mut self) -> Self::State;

    fn render(&self, state: &Self::State) -> String;
}

pub struct PolybarModuleEnv {
    pub low_bw_filepath: PathBuf,
    pub public_screen_filepath: PathBuf,
    pub network_error_backoff: ExponentialBackoff,
}

impl PolybarModuleEnv {
    pub fn new() -> Self {
        let xdg_dirs = xdg::BaseDirectories::new().unwrap();
        let low_bw_filepath = xdg_dirs.get_data_home().join("low_internet_bandwidth");
        let public_screen_filepath = xdg_dirs.place_runtime_file("public_screen").unwrap();
        let network_error_backoff = ExponentialBackoffBuilder::new()
            .with_initial_interval(Duration::from_secs(5))
            .with_randomization_factor(0.25)
            .with_multiplier(1.5)
            .with_max_interval(Duration::from_secs(60 * 60))
            .with_max_elapsed_time(None)
            .build();
        Self {
            low_bw_filepath,
            public_screen_filepath,
            network_error_backoff,
        }
    }

    pub fn network_mode(&self) -> NetworkMode {
        match self.low_bw_filepath.exists() {
            true => NetworkMode::LowBandwith,
            false => NetworkMode::Unrestricted,
        }
    }

    pub fn public_screen(&self) -> bool {
        self.public_screen_filepath.exists()
    }

    pub fn wait_network_mode(&self, mode: NetworkMode) -> bool {
        let mut did_wait = false;
        let (events_tx, events_rx) = channel();
        let mut watcher = notify::watcher(events_tx, Duration::from_millis(10)).unwrap();
        let parent_dir = self.low_bw_filepath.parent().unwrap();
        log::debug!("Watching {:?}", parent_dir);
        watcher
            .watch(parent_dir, notify::RecursiveMode::NonRecursive)
            .unwrap();
        while self.network_mode() != mode {
            let evt = events_rx.recv().unwrap();
            did_wait = true;
            log::trace!("{:?}", evt);
        }
        did_wait
    }

    pub fn wait_public_screen(&self, public: bool) -> bool {
        let mut did_wait = false;
        let (events_tx, events_rx) = channel();
        let mut watcher = notify::watcher(events_tx, Duration::from_millis(10)).unwrap();
        let parent_dir = self.public_screen_filepath.parent().unwrap();
        log::debug!("Watching {:?}", parent_dir);
        watcher
            .watch(parent_dir, notify::RecursiveMode::NonRecursive)
            .unwrap();
        while self.public_screen() != public {
            let evt = events_rx.recv().unwrap();
            did_wait = true;
            log::trace!("{:?}", evt);
        }
        did_wait
    }
}
