//! Injects the build's git commit hash into the binary as `HONEYPOT_BUILD_SHA`,
//! surfaced by the `/version` command.
//!
//! Docker/CI builds pass the SHA via the `HONEYPOT_BUILD_SHA` environment
//! variable (a build-arg), which takes precedence so the hash is deterministic
//! even when the `.git` directory or the `git` binary is absent from the build
//! image. A local `cargo build` falls back to shelling out to `git`; anything
//! else resolves to `unknown` rather than failing the build (keeps musl/Windows
//! cross-builds and source-only checkouts green).

use std::process::Command;

fn main() {
    let sha = std::env::var("HONEYPOT_BUILD_SHA")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(git_short_sha)
        .unwrap_or_else(|| "unknown".to_string());

    // A full 40-char SHA (as CI passes) is trimmed to a readable prefix; a
    // `git --short` value is already shorter and passes through untouched.
    let sha: String = sha.chars().take(12).collect();

    println!("cargo:rustc-env=HONEYPOT_BUILD_SHA={sha}");
    println!("cargo:rerun-if-env-changed=HONEYPOT_BUILD_SHA");
    // `.git/HEAD` changes on branch switch; `.git/logs/HEAD` is appended to on
    // every commit/checkout/reset, so it — not HEAD, whose content is unchanged
    // by a commit on the same branch — is what refreshes the hash after a commit.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/logs/HEAD");
}

/// The current commit's short SHA via `git`, or `None` when git is unavailable.
fn git_short_sha() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!sha.is_empty()).then_some(sha)
}
