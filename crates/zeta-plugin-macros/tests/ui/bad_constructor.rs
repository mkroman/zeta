//! A constructor parameter that is not one of the recognized types is almost always a typo.

use zeta_plugin::plugin;

#[derive(Default)]
struct Settings;

#[derive(Default)]
struct BadConstructor;

#[plugin(name = "bad_constructor", settings = Settings)]
impl BadConstructor {
    fn new(
        _: &zeta_plugin::Context,
        _: &Settings,
        _bogus: u32,
    ) -> Result<Self, zeta_plugin::Error> {
        Ok(Self)
    }
}

fn main() {}
