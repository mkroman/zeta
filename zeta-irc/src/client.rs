/// Structure that holds client state information, such as connections
///
/// A single client can have multiple connections
pub struct Client {}

#[cfg(feature = "builder")]
#[derive(Default)]
pub struct ClientBuilder {}

#[cfg(feature = "builder")]
impl ClientBuilder {
    /// Constructs a new ClientBuilder
    pub fn new() -> ClientBuilder {
        Default::default()
    }

    /// Builds the final Client
    pub fn build() -> Client {
        Client {}
    }
}

impl Client {
    /// Returns a new ClientBuilder
    pub fn build() -> ClientBuilder {
        ClientBuilder::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "builder")]
    #[test]
    fn it_should_build() {
        Client::build();
    }
}
