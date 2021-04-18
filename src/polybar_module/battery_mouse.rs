use crate::polybar_module::StatefulPolybarModule;

pub struct BatteryMouseModule {}

#[derive(Debug, PartialEq)]
pub struct BatteryMouseModuleState {}

impl BatteryMouseModule {
    pub fn new() -> BatteryMouseModule {
        BatteryMouseModule {}
    }
}

impl StatefulPolybarModule for BatteryMouseModule {
    type State = BatteryMouseModuleState;

    fn wait_update(&mut self, _first_update: bool) {}

    fn update(&mut self) -> Self::State {
        Self::State {}
    }

    fn render(&self, _state: &Self::State) -> String {
        "".to_string()
    }
}
