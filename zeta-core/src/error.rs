use failure::Fail;

#[derive(Fail, Debug)]
pub enum Error {
    #[fail(display = "IRC error: {}", _0)]
    IrcError(#[fail(cause)] irc::error::Error),
    /// Indicates that the client has not been connected
    #[fail(display = "Client not initialized or connected")]
    ClientNotConnectedError,
}

impl From<irc::error::Error> for Error {
    fn from(err: irc::error::Error) -> Error {
        Error::IrcError(err)
    }
}
