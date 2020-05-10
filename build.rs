// build.rs
use std::process::Command;
fn main() {
    let git_hash = if let Ok(output) = Command::new("git").args(&["rev-parse", "HEAD"]).output() {
        String::from_utf8(output.stdout).unwrap()
    } else {
        "unknown".to_string()
    };
    println!(
        "cargo:rustc-env=SOLAR_VERSION={}-{}",
        env!("CARGO_PKG_VERSION"),
        &git_hash[..7]
    );
}
