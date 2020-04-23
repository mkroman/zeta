use zeta_core::{Core, Error};

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Initialize logging
    env_logger::init();

    let mut core = Core::new();

    core.connect().await?;
    core.poll().await?;

    Ok(())
}
