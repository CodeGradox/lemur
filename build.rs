//! Builds lemur-dictate, the Swift helper dictation runs, for `cargo run`
//! and the tests; scripts/build-app.sh puts its own build in lemur.app.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/dictate.swift");
    println!("cargo:rerun-if-changed=scripts/build-dictate.sh");
    let helper = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("lemur-dictate");
    let status = Command::new("sh")
        .arg("scripts/build-dictate.sh")
        .arg(&helper)
        .status()
        .expect("running scripts/build-dictate.sh");
    assert!(status.success(), "scripts/build-dictate.sh failed");
    println!("cargo:rustc-env=LEMUR_DICTATE={}", helper.display());
}
