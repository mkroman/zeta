//! The plugin [`Context`](zeta_plugin::Context) helper, the mock IRC client, and a recording
//! plugin the runtime tests drive through the lifecycle.

/// Constructs a plugin [`Context`](zeta_plugin::Context) for tests: a default configuration
/// and an unreachable (lazily connected) database pool.
///
/// # Panics
///
/// Panics if the lazy database pool cannot be constructed, which only happens with a
/// misconfigured sqlx build.
#[must_use]
pub fn test_context() -> zeta_plugin::Context {
    zeta_plugin::Context::new(
        {
            use sqlx::postgres::PgPoolOptions;

            PgPoolOptions::new()
                .connect_lazy("postgresql://invalid/zeta_test")
                .expect("lazy database pool")
        },
        zeta_plugin::config::HostConfig::default(),
    )
}

/// Builds an IRC client on the `irc` crate's mock connection, so a test needs no server.
///
/// # Panics
///
/// Panics if the mock client cannot be constructed.
pub async fn mock_client() -> zeta_plugin::Client {
    zeta_plugin::Client::from_config(irc::client::data::Config {
        nickname: Some("zeta-test".to_owned()),
        server: Some("mock".to_owned()),
        use_mock_connection: true,
        mock_initial_value: Some(String::new()),
        ..Default::default()
    })
    .await
    .expect("mock irc client")
}

/// The lifecycle log shared between a test and its [`RecordingPlugin`].
pub type Recording = std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>;

/// Publishes a fresh recording log through the context's shared state.
///
/// The returned handle observes the lifecycle events [`RecordingPlugin`] appends.
///
/// # Panics
///
/// Panics if the context already holds a recording log.
pub fn publish_recording(ctx: &zeta_plugin::Context) -> Recording {
    let recording: Recording = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

    assert!(
        ctx.shared()
            .publish(std::sync::Arc::clone(&recording))
            .is_none()
    );

    recording
}

/// A plugin recording the lifecycle events it observes in a shared log.
///
/// It registers one command (`.recording`) so registries can exercise command publication, and
/// appends `"message"` and `"shutdown"` to the log from the matching handlers.
pub struct RecordingPlugin {
    /// The log the plugin appends to, shared with the test.
    pub recording: Recording,
}

// The trait's handlers are awaited by the plugin runtime; the recording only appends to a log,
// so there is nothing to `.await` inside them.
#[allow(clippy::unused_async_trait_impl)]
impl zeta_plugin::Plugin for RecordingPlugin {
    async fn handle<'a>(
        &'a self,
        _: &'a zeta_plugin::Context,
        _: &'a zeta_plugin::Client,
        event: &'a zeta_plugin::Event,
    ) -> Result<(), zeta_plugin::Error> {
        if let zeta_plugin::Event::Message(_) = event {
            self.recording
                .lock()
                .expect("recording log lock")
                .push("message");
        }

        Ok(())
    }

    async fn shutdown<'a>(
        &'a mut self,
        _: &'a zeta_plugin::Context,
        _: &'a zeta_plugin::Client,
    ) -> Result<(), zeta_plugin::Error> {
        self.recording
            .lock()
            .expect("recording log lock")
            .push("shutdown");

        Ok(())
    }
}

impl zeta_plugin::Registered for RecordingPlugin {
    const NAME: &'static str = "recording";

    type Settings = zeta_plugin::NoSettings;

    fn new(
        ctx: &zeta_plugin::Context,
        _: &zeta_plugin::NoSettings,
        subscriptions: &mut zeta_plugin::Subscriptions,
    ) -> Result<Self, zeta_plugin::Error> {
        subscriptions.command(zeta_plugin::CommandSpec::new(
            ".recording",
            "record a message",
        ));

        let recording = ctx
            .shared()
            .get::<std::sync::Mutex<Vec<&'static str>>>()
            .expect("recording log should be published through `publish_recording`");

        Ok(Self { recording })
    }
}
