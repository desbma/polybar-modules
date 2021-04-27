use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::time::Duration;

use notify::Watcher;

pub mod arch_updates;
pub mod autolock;
pub mod battery_mouse;
pub mod debian_updates;
pub mod gpu_nvidia;
pub mod internet_bandwidth;
pub mod market;
pub mod network_status;
pub mod pulseaudio;
pub mod taskwarrior;
pub mod wttr;

pub enum PolybarModule {
    ArchUpdates(arch_updates::ArchUpdatesModule),
    Autolock(autolock::AutolockModule),
    BatteryMouse(battery_mouse::BatteryMouseModule),
    DebianUpdates(debian_updates::DebianUpdatesModule),
    GpuNvidia(gpu_nvidia::GpuNvidiaModule),
    InternetBandwidth(internet_bandwidth::InternetBandwidthModule),
    Market(market::MarketModule),
    NetworkStatus(network_status::NetworkStatusModule),
    PulseAudio(pulseaudio::PulseAudioModule),
    Taskwarrior(taskwarrior::TaskwarriorModule),
    Wttr(wttr::WttrModule),
}

#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeMode {
    Unrestricted,
    LowNetworkBandwith,
}

pub trait RenderablePolybarModule {
    type State: Debug + PartialEq;

    fn wait_update(&mut self, prev_state: &Option<Self::State>);

    fn update(&mut self) -> Self::State;

    fn render(&self, state: &Self::State) -> String;
}

pub struct PolybarModuleEnv {
    pub low_bw_filepath: PathBuf,
}

impl PolybarModuleEnv {
    pub fn new() -> PolybarModuleEnv {
        let xdg_dirs = xdg::BaseDirectories::new().unwrap();
        let low_bw_filepath = xdg_dirs.get_data_home().join("low_internet_bandwidth");
        PolybarModuleEnv { low_bw_filepath }
    }

    pub fn get_runtime_mode(&self) -> RuntimeMode {
        match self.low_bw_filepath.exists() {
            true => RuntimeMode::LowNetworkBandwith,
            false => RuntimeMode::Unrestricted,
        }
    }

    pub fn wait_runtime_mode(&self, mode: RuntimeMode) -> bool {
        let mut did_wait = false;
        let (events_tx, events_rx) = channel();
        let mut watcher = notify::watcher(events_tx, Duration::from_millis(10)).unwrap();
        let parent_dir = self.low_bw_filepath.parent().unwrap();
        log::debug!("Watching {:?}", parent_dir);
        watcher
            .watch(parent_dir, notify::RecursiveMode::NonRecursive)
            .unwrap();
        while self.get_runtime_mode() != mode {
            let evt = events_rx.recv().unwrap();
            did_wait = true;
            log::trace!("{:?}", evt);
        }
        did_wait
    }
}
