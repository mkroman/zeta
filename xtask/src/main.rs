//! The `cargo xtask` entry point.

fn main() {
    if let Err(error) = xtask::run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
