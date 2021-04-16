pub mod battery_mouse;
pub mod wttr;

pub enum PolybarModule {
    BatteryMouse(battery_mouse::BatteryMouseModule),
    Wttr(wttr::WttrModule),
}

pub trait StatefulPolybarModule {
    type State: std::fmt::Debug + PartialEq;

    fn wait_update(&mut self);

    fn update(&mut self) -> Self::State;

    fn render(&self, state: &Self::State) -> String;
}
