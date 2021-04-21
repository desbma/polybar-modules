use config::PolybarModuleName;
use structopt::StructOpt;

mod config;
mod markup;
mod polybar_module;
mod theme;

fn main() {
    // Init logger
    if atty::is(atty::Stream::Stdout) {
        simple_logger::SimpleLogger::new().init().unwrap();
    }

    // Parse command line args
    let opts = config::CommandLineOpts::from_args();
    log::trace!("{:?}", opts);

    // Init stuff
    let module: polybar_module::PolybarModule = match opts.module {
        PolybarModuleName::autolock => {
            polybar_module::PolybarModule::Autolock(polybar_module::autolock::AutolockModule::new())
        }
        PolybarModuleName::battery_mouse => polybar_module::PolybarModule::BatteryMouse(
            polybar_module::battery_mouse::BatteryMouseModule::new(),
        ),
        PolybarModuleName::gpu_nvidia => polybar_module::PolybarModule::GpuNvidia(
            polybar_module::gpu_nvidia::GpuNvidiaModule::new(),
        ),
        PolybarModuleName::internet_bandwidth => polybar_module::PolybarModule::InternetBandwidth(
            polybar_module::internet_bandwidth::InternetBandwidthModule::new(),
        ),
        PolybarModuleName::pulseaudio => polybar_module::PolybarModule::PulseAudio(
            polybar_module::pulseaudio::PulseAudioModule::new(),
        ),
        PolybarModuleName::wttr { location } => {
            polybar_module::PolybarModule::Wttr(polybar_module::wttr::WttrModule::new(location))
        }
    };

    // Update/render loop, dynamic dispatch sadness, sadly https://crates.io/crates/enum_dispatch does not work here
    match module {
        polybar_module::PolybarModule::Autolock(module) => render_loop(module),
        polybar_module::PolybarModule::BatteryMouse(module) => render_loop(module),
        polybar_module::PolybarModule::GpuNvidia(module) => render_loop(module),
        polybar_module::PolybarModule::InternetBandwidth(module) => render_loop(module),
        polybar_module::PolybarModule::PulseAudio(module) => render_loop(module),
        polybar_module::PolybarModule::Wttr(module) => render_loop(module),
    };
}

fn render_loop<T>(mut module: T)
where
    T: polybar_module::RenderablePolybarModule,
{
    let mut prev_state: Option<T::State> = None;
    loop {
        // Update
        module.wait_update(prev_state.is_none());
        let state = module.update();
        log::debug!("{:?}", state);

        // Render or skip?
        let do_render = match prev_state {
            Some(ref prev_state) => prev_state != &state,
            None => true,
        };
        if !do_render {
            continue;
        }

        // Render
        let output = module.render(&state);
        println!("{}", output);
        prev_state = Some(state);
    }
}
