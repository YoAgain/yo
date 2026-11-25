//! The rules of `skills/`, without a hub.

use std::process::Command;

#[test]
fn the_skills_rules_hold_without_a_hub() {
    let skills = concat!(env!("CARGO_MANIFEST_DIR"), "/../../skills");
    let status = Command::new("python3")
        .args(["-B", "-m", "unittest", "discover", "-s", skills, "-p", "test_*.py"])
        .status()
        .unwrap();
    assert!(status.success());
}
