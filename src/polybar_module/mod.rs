use std::{fmt::Debug, path::PathBuf, sync::mpsc::channel, time::Duration};

use backoff::{ExponentialBackoff, ExponentialBackoffBuilder};
use notify::Watcher as _;

pub(crate) mod arch_updates;
pub(crate) mod autolock;
pub(crate) mod battery_mouse;
pub(crate) mod bluetooth;
pub(crate) mod cpu_freq;
pub(crate) mod cpu_top;
pub(crate) mod debian_updates;
pub(crate) mod gpu_nvidia;
pub(crate) mod home_power;
pub(crate) mod internet_bandwidth;
pub(crate) mod market;
pub(crate) mod network_status;
pub(crate) mod player;
pub(crate) mod progressbar_server;
pub(crate) mod pulseaudio;
pub(crate) mod syncthing;
mod syncthing_rest;
pub(crate) mod todotxt;
pub(crate) mod wttr;
pub(crate) mod xmonad;

pub(crate) enum PolybarModule {
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
    Player(player::PlayerModule),
    ProgressBarServer(progressbar_server::ProgressBarServerModule),
    PulseAudio(pulseaudio::PulseAudioModule),
    Syncthing(syncthing::SyncthingModule),
    TodoTxt(todotxt::TodoTxtModule),
    Wttr(wttr::WttrModule),
    Xmonad(xmonad::XmonadModule),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NetworkMode {
    Unrestricted,
    LowBandwith,
}

const TCP_REMOTE_TIMEOUT: Duration = Duration::from_secs(20);
const TCP_LOCAL_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) trait RenderablePolybarModule {
    type State: Debug + PartialEq;

    fn wait_update(&mut self, prev_state: Option<&Self::State>);

    fn update(&mut self) -> Self::State;

    fn render(&self, state: &Self::State) -> String;
}

pub(crate) struct PolybarModuleEnv {
    pub low_bw_filepath: PathBuf,
    pub public_screen_filepath: PathBuf,
    pub network_error_backoff: ExponentialBackoff,
}

impl PolybarModuleEnv {
    pub(crate) fn new() -> Self {
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

    pub(crate) fn network_mode(&self) -> NetworkMode {
        if self.low_bw_filepath.exists() {
            NetworkMode::LowBandwith
        } else {
            NetworkMode::Unrestricted
        }
    }

    pub(crate) fn public_screen(&self) -> bool {
        self.public_screen_filepath.exists()
    }

    pub(crate) fn wait_network_mode(&self, mode: &NetworkMode) -> bool {
        let mut did_wait = false;
        let (events_tx, events_rx) = channel();
        let mut watcher = notify::recommended_watcher(events_tx).unwrap();
        watcher
            .watch(&self.low_bw_filepath, notify::RecursiveMode::NonRecursive)
            .unwrap();
        log::debug!("Watching {:?}", self.low_bw_filepath);
        while self.network_mode() != *mode {
            let evt = events_rx.recv().unwrap();
            did_wait = true;
            log::trace!("{:?}", evt);
        }
        did_wait
    }

    pub(crate) fn wait_public_screen(&self, public: bool) -> bool {
        let mut did_wait = false;
        let (events_tx, events_rx) = channel();
        let mut watcher = notify::recommended_watcher(events_tx).unwrap();
        watcher
            .watch(
                &self.public_screen_filepath,
                notify::RecursiveMode::NonRecursive,
            )
            .unwrap();
        log::debug!("Watching {:?}", self.public_screen_filepath);
        while self.public_screen() != public {
            let evt = events_rx.recv().unwrap();
            did_wait = true;
            log::trace!("{:?}", evt);
        }
        did_wait
    }
}
