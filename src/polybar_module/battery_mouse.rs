//use crate::markup;
use crate::polybar_module::RenderablePolybarModule;
//use crate::theme;

pub struct BatteryMouseModule {}

#[derive(Debug, PartialEq)]
pub struct BatteryMouseModuleState {}

impl BatteryMouseModule {
    pub fn new() -> BatteryMouseModule {
        BatteryMouseModule {}
    }
}

impl RenderablePolybarModule for BatteryMouseModule {
    type State = BatteryMouseModuleState;

    fn wait_update(&mut self, _prev_state: &Option<Self::State>) {}

    fn update(&mut self) -> Self::State {
        Self::State {}
    }

    fn render(&self, _state: &Self::State) -> String {
        "".to_string()
    }
}

#[cfg(test)]
mod tests {
    //use super::*;

    #[test]
    fn test_render() {
        // TODO
    }
}
