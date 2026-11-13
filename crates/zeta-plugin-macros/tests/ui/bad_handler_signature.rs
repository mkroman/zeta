//! The event parameter is required and must be the kind's event type.

use zeta_plugin::plugin;

#[derive(Default)]
struct BadSignature;

#[plugin(name = "bad_signature")]
impl BadSignature {
    #[handler(url)]
    async fn url(
        &self,
        _: &zeta_plugin::Context,
        _: &zeta_plugin::Client,
    ) -> Result<(), zeta_plugin::Error> {
        Ok(())
    }
}

fn main() {}
