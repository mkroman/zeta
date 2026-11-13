//! An unrecognized `#[handler(kind)]` must list the kinds the macro accepts.

use zeta_plugin::plugin;

#[derive(Default)]
struct Bad;

#[plugin(name = "bad")]
impl Bad {
    #[handler(bogus)]
    async fn handler(
        &self,
        _: &zeta_plugin::Context,
        _: &zeta_plugin::Client,
        _: &zeta_plugin::Event,
    ) -> Result<(), zeta_plugin::Error> {
        Ok(())
    }
}

fn main() {}
