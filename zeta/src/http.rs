//! HTTP features

mod client {
    use crate::consts;

    pub use reqwest::Client;

    /// Returns a default HTTP client.
    ///
    /// # Panics
    ///
    /// Panics if the default HTTP client fails to build.
    #[must_use]
    pub fn build() -> Client {
        builder().build().expect("could not build http client")
    }

    /// Returns a default HTTP client builder.
    pub fn builder() -> reqwest::ClientBuilder {
        reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(consts::HTTP_TIMEOUT)
            .user_agent(consts::HTTP_USER_AGENT)
    }
}

/// Builds a default HTTP client.
///
/// This is equivalent to calling [`client::build`].
pub fn build_client() -> client::Client {
    client::build()
}
