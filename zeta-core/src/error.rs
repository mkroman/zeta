use failure::Fail;

#[derive(Fail, Debug)]
pub enum Error {
    /// Indicates that the client has not been connected
    #[fail(display = "Client not initialized or connected")]
    ClientNotConnectedError,
}
