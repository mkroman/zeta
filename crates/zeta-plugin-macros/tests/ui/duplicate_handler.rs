//! Two methods handling the same event kind are ambiguous and must be rejected.

use zeta_plugin::plugin;

#[derive(Default)]
struct Duplicate;

#[plugin(name = "duplicate")]
impl Duplicate {
    #[handler(message)]
    async fn first(
        &self,
        _: &zeta_plugin::Context,
        _: &zeta_plugin::Client,
        _: &zeta_plugin::MessageEvent,
    ) -> Result<(), zeta_plugin::Error> {
        Ok(())
    }

    #[handler(message)]
    async fn second(
        &self,
        _: &zeta_plugin::Context,
        _: &zeta_plugin::Client,
        _: &zeta_plugin::MessageEvent,
    ) -> Result<(), zeta_plugin::Error> {
        Ok(())
    }
}

fn main() {}
