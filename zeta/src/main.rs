use zeta_core::{Core, Error};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut core = Core::new();

    core.connect().await?;
    core.poll().await?;

    Ok(())
}
