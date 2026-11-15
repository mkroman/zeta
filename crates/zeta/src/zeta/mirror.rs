//! Startup of the optional shared media mirror.
//!
//! `start_mirror` starts the download manager of the mirror published on the context, before
//! the IRC connection is established.

/// Starts the download manager of the shared mirror, when one is published.
///
/// The mirror is published to the context's shared state during construction; plugins pick up
/// their handles from there, and the host starts the manager so the first mirrored URL does not
/// pay for the manager's synchronous filesystem setup.
///
/// Returns whether the manager is running; this is `false` when no mirror is published or its
/// download directory could not be created, in which case mirroring stays disabled.
pub(super) fn start_mirror(shared: &zeta_plugin::SharedState) -> bool {
    shared
        .get::<zeta_mirror::Mirror>()
        .is_some_and(|mirror| mirror.start())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// A mirror whose manager can be started without network access.
    fn test_mirror(download_dir: &std::path::Path) -> Arc<zeta_mirror::Mirror> {
        let config = zeta_mirror::MirrorConfig {
            download_dir: Some(download_dir.to_path_buf()),
            s3_access_key_id: Some("access".to_string()),
            s3_secret_access_key: Some("secret".into()),
            s3_bucket_name: Some("bucket".to_string()),
            ..zeta_mirror::MirrorConfig::default()
        };

        Arc::new(
            zeta_mirror::Mirror::from_config(&config)
                .expect("the test configuration should be valid"),
        )
    }

    #[tokio::test]
    async fn start_mirror_starts_the_published_manager() {
        let dir = tempfile::tempdir().unwrap();
        let shared = zeta_plugin::SharedState::new();

        assert!(
            !start_mirror(&shared),
            "Nothing is published, so there is nothing to start."
        );

        shared.publish(test_mirror(dir.path()));
        assert!(
            start_mirror(&shared),
            "startup should start the published mirror's manager"
        );

        assert!(start_mirror(&shared), "A second startup call is a no-op.");
    }
}
