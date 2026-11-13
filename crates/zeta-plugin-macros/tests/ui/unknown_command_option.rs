//! An unrecognized `#[command(...)]` option must list the options the macro accepts.

use zeta_plugin::plugin;

#[derive(Default)]
struct BadOption;

#[plugin(name = "bad_option")]
impl BadOption {
    #[command(".bad", "A command", bogus = 1)]
    async fn bad(
        &self,
        _: &zeta_plugin::Context,
        _: &zeta_plugin::Client,
        _channel: &str,
    ) -> Result<(), zeta_plugin::Error> {
        Ok(())
    }
}

fn main() {}
