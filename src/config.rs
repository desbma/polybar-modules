use arg_enum_proc_macro::ArgEnum;
use structopt::StructOpt;

#[derive(ArgEnum, Clone, Debug)]
#[allow(non_camel_case_types)]
pub enum PolybarModuleName {
    battery_mouse,
    wttr,
}

#[derive(StructOpt, Debug)]
#[structopt(version=env!("CARGO_PKG_VERSION"), about="Polybar modules.")]
pub struct CommandLineOpts {
    #[structopt(
        possible_values = &PolybarModuleName::variants(),
        case_insensitive = true
    )]
    pub module: PolybarModuleName,
}
