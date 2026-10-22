//! `/version` — reports the crate version and the build's commit hash.
//!
//! The commit hash is baked in at compile time by `build.rs` as
//! `HONEYPOT_BUILD_SHA` (see that file for how it is resolved). Both fields link
//! into GitHub — the version to its release tag, the hash to its commit.

use crate::discord::commands;
use crate::i18n::Language;
use twilight_http::Client;
use twilight_model::application::interaction::Interaction;
use twilight_model::channel::message::Embed;
use twilight_util::builder::embed::{EmbedBuilder, EmbedFieldBuilder};

/// The build's commit hash, injected by `build.rs`.
const BUILD_SHA: &str = env!("HONEYPOT_BUILD_SHA");
/// The crate version.
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The repository URL, from `Cargo.toml`.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
/// Sentinel `build.rs` emits when it cannot resolve a commit hash.
const UNKNOWN_SHA: &str = "unknown";

pub(super) async fn run(http: &Client, interaction: &Interaction) {
    let language = commands::language_for(interaction.guild_id);
    commands::respond_embed(http, interaction, build_embed(language)).await;
}

/// Builds the version embed for `language`.
pub(crate) fn build_embed(language: Language) -> Embed {
    let msg = language.messages();
    EmbedBuilder::new()
        .title(msg.version_title)
        .field(EmbedFieldBuilder::new(msg.version_label, version_link()).inline())
        .field(EmbedFieldBuilder::new(msg.build_label, build_link()).inline())
        .build()
}

/// The version as a markdown link to its GitHub release tag.
fn version_link() -> String {
    // release-please tags this crate as `honeypot-v{version}`.
    format!("[{VERSION}]({REPOSITORY}/releases/tag/honeypot-v{VERSION})")
}

/// The `build.rs`-injected commit hash as a GitHub commit link.
fn build_link() -> String {
    build_link_for(BUILD_SHA)
}

/// The commit hash as a markdown link to its GitHub commit, or plain text when
/// the hash is unknown (a build without git or an injected SHA).
fn build_link_for(sha: &str) -> String {
    if sha == UNKNOWN_SHA {
        format!("`{sha}`")
    } else {
        format!("[`{sha}`]({REPOSITORY}/commit/{sha})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_link_points_at_the_release_tag() {
        let link = version_link();
        assert!(link.contains(REPOSITORY));
        assert!(link.contains(VERSION));
        assert!(link.contains("/releases/tag/honeypot-v"));
    }

    #[test]
    fn build_link_links_a_known_hash_to_its_commit() {
        let link = build_link_for("abc1234");
        assert_eq!(link, format!("[`abc1234`]({REPOSITORY}/commit/abc1234)"));
    }

    #[test]
    fn build_link_is_plain_text_for_an_unknown_hash() {
        assert_eq!(build_link_for(UNKNOWN_SHA), "`unknown`");
    }
}
