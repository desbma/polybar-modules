use crate::polybar_module::StatefulPolybarModule;

pub struct WttrModule {}

#[derive(Debug, PartialEq)]
pub struct WttrModuleState {}

impl WttrModule {
    pub fn new() -> WttrModule {
        WttrModule {}
    }
}

impl StatefulPolybarModule for WttrModule {
    type State = WttrModuleState;

    fn wait_update(&mut self) {}

    fn update(&mut self) -> Self::State {
        Self::State {}
    }

    fn render(&self, state: &Self::State) -> String {
        "".to_string()
    }
}
