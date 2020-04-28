use std::path::Path;
use std::process::Command;
use std::{env, fs};

fn main() {
    let target_dir = Path::new(&env::var("OUT_DIR").unwrap()).to_path_buf();

    let file = fs::File::create(target_dir.join("git_commit")).unwrap();

    // Write the commit hash to the file
    Command::new("git")
        .args(&["rev-parse", "HEAD"])
        .stdout(file)
        .spawn()
        .expect("failed to get git commit hash");
}
