//! `usage` is the complete reply line and `usage_hint` only its arguments, so declaring both
//! must be rejected instead of silently picking one.

use zeta_plugin::plugin;

#[derive(Default)]
struct BothUsageForms;

#[plugin(name = "both_usage_forms")]
impl BothUsageForms {
    #[command(
        ".both",
        "A command",
        usage = "Usage: .both <query>",
        usage_hint = "<query>"
    )]
    async fn both(
        &self,
        _: &zeta_plugin::Context,
        _: &zeta_plugin::Client,
        _channel: &str,
    ) -> Result<(), zeta_plugin::Error> {
        Ok(())
    }
}

fn main() {}
