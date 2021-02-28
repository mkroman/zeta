use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    /// Indicates that the client has not been connected
    #[error("Client not initialized or connected")]
    ClientNotConnectedError,
}
