//! A plugin cannot combine `#[command]` methods with `#[handler(command)]`.

use zeta_plugin::plugin;

#[derive(Default)]
struct Conflict;

#[plugin(name = "conflict")]
impl Conflict {
    #[command(".conflict", "A command")]
    async fn command(
        &self,
        _: &zeta_plugin::Context,
        _: &zeta_plugin::Client,
        _channel: &str,
    ) -> Result<(), zeta_plugin::Error> {
        Ok(())
    }

    #[handler(command)]
    async fn dynamic(
        &self,
        _: &zeta_plugin::Context,
        _: &zeta_plugin::Client,
        _: &zeta_plugin::CommandEvent,
    ) -> Result<(), zeta_plugin::Error> {
        Ok(())
    }
}

fn main() {}
