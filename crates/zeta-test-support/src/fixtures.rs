//! The `fixture!` macro reading a consuming crate's test fixtures.

/// Reads a fixture file from the calling crate's `tests/fixtures` directory.
///
/// The path is resolved against the crate that invokes the macro, so plugins can keep their
/// fixtures next to their tests without repeating the manifest-directory dance.
///
/// # Panics
///
/// Panics if the fixture cannot be read, naming the resolved path and the cause.
#[macro_export]
macro_rules! fixture {
    ($path:expr) => {{
        let path = ::std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join($path);

        ::std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("could not read fixture {}: {error}", path.display()))
    }};
}
