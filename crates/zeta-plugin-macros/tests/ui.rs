//! Compile-fail coverage for the `#[plugin]` macro's diagnostics.
//!
//! The inline tests assert the classification logic; this test compiles broken plugins and
//! snapshots rustc's actual diagnostics, so a change to the macro's error reporting is visible
//! in review instead of silently degrading the message a plugin author sees.
//!
//! The snapshots are pinned to the stable compiler. Nightly renders an attribute macro's span
//! as the whole attribute (`^^^^^^^^^^^^^^^^^^^`) where stable renders a single caret (`^`), so
//! one snapshot cannot match both channels. Stable is the release toolchain and the gate; the
//! nightly job still compiles the macro and its generated tests.

#[test]
fn ui() {
    if is_nightly() {
        return;
    }

    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}

/// Whether the compiler running this test is a nightly build.
fn is_nightly() -> bool {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("nightly"))
}
