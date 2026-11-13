//! Compile-fail coverage for the `#[plugin]` macro's diagnostics.
//!
//! The inline tests assert the classification logic; this test compiles broken plugins and
//! snapshots rustc's actual diagnostics, so a change to the macro's error reporting is visible
//! in review instead of silently degrading the message a plugin author sees.

#[test]
fn ui() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}
