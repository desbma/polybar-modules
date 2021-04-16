use crate::polybar_module::StatefulPolybarModule;

pub struct BatteryMouseModule {}

#[derive(Debug, PartialEq)]
pub struct BatteryMouseModuleState {}

impl BatteryMouseModule {
    pub fn new() -> BatteryMouseModule {
        BatteryMouseModule {}
    }
}

// TODO implement a way to pass low bandwidth flag
impl StatefulPolybarModule for BatteryMouseModule {
    type State = BatteryMouseModuleState;

    fn wait_update(&mut self) {}

    fn update(&mut self) -> Self::State {
        Self::State {}
    }

    fn render(&self, state: &Self::State) -> String {
        "".to_string()
    }
}
