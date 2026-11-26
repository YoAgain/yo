//! `yo version`: the release it was built for. `YO_VERSION` (the CLI workflow sets it from the release tag), else
//! the last release in app/android/version.properties (`versionName`), which dev/release.sh bumps.
fn main() {
    const PROPERTIES: &str = "../android/version.properties";
    println!("cargo:rerun-if-env-changed=YO_VERSION");
    println!("cargo:rerun-if-changed={PROPERTIES}");
    let version = std::env::var("YO_VERSION").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| {
        std::fs::read_to_string(PROPERTIES)
            .ok()
            .and_then(|text| text.lines().find_map(|l| l.strip_prefix("versionName=").map(|v| v.trim().to_owned())))
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned())
    });
    println!("cargo:rustc-env=YO_VERSION={}", version.trim_start_matches('v'));
}
