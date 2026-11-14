use std::process::Command;
fn main() {
    println!("cargo:rerun-if-env-changed=WEBTEST_BUILD_COMMIT");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs");
    let commit = std::env::var("WEBTEST_BUILD_COMMIT")
        .ok()
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
        })
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=WEBTEST_BUILD_COMMIT={}", commit.trim());
    println!(
        "cargo:rustc-env=WEBTEST_BUILD_TARGET={}",
        std::env::var("TARGET").unwrap_or_default()
    );
}
