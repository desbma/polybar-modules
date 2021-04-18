use notify::Watcher;

pub mod battery_mouse;
pub mod gpu_nvidia;
pub mod wttr;

pub enum PolybarModule {
    BatteryMouse(battery_mouse::BatteryMouseModule),
    GpuNvidia(gpu_nvidia::GpuNvidiaModule),
    Wttr(wttr::WttrModule),
}

#[derive(PartialEq)]
pub enum RuntimeMode {
    Unrestricted,
    LowNetworkBandwith,
}

pub trait StatefulPolybarModule {
    type State: std::fmt::Debug + PartialEq;

    fn wait_update(&mut self, first_update: bool);

    fn update(&mut self) -> Self::State;

    fn render(&self, state: &Self::State) -> String;
}

pub struct PolybarModuleEnv {
    low_bw_filepath: std::path::PathBuf,
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

    pub fn wait_runtime_mode(&self, mode: RuntimeMode) {
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        let mut watcher = notify::raw_watcher(events_tx).unwrap();
        let parent_dir = self.low_bw_filepath.parent().unwrap();
        log::debug!("Watching {:?}", parent_dir);
        watcher
            .watch(parent_dir, notify::RecursiveMode::NonRecursive)
            .unwrap();
        while self.get_runtime_mode() != mode {
            let evt = events_rx.recv().unwrap();
            log::trace!("{:?}", evt);
        }
    }
}
