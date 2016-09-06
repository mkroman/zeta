// Build script to automatically build plugins when the core is built.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    if env::var("BUILD_PLUGINS").is_ok() {
        return;
    }

    // let out_dir = env::var("OUT_DIR").unwrap();
    let manifest_dir = PathBuf::from(&env::var("CARGO_MANIFEST_DIR").unwrap());
    let plugins_manifest_dir = manifest_dir.as_path().join("plugins");

    //panic!("{:?}", env::current_dir().unwrap());

    let mut cargo = Command::new("cargo");

    cargo.arg("build").current_dir(plugins_manifest_dir).env("BUILD_PLUGINS", "1");

    if env::var("PROFILE").unwrap() == "release" {
        cargo.arg("--release");
    }

    cargo.output().expect("failed to build plugins");
}
