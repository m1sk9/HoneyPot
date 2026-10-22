//! Blank model fixtures for tests and the embed preview.
//!
//! twilight's [`User`] and [`Embed`] implement no `Default`, so these build the
//! minimal values the test suites and `crate::discord::preview` start from and
//! then override field by field.

use twilight_model::channel::message::Embed;
use twilight_model::id::Id;
use twilight_model::user::User;

/// A plain (non-bot, non-system) user with no avatar, flags, or display name.
///
/// `discriminator` is `0`, as for every account migrated to unique usernames.
pub(crate) fn user(id: u64, name: &str) -> User {
    User {
        accent_color: None,
        avatar: None,
        avatar_decoration: None,
        avatar_decoration_data: None,
        banner: None,
        bot: false,
        discriminator: 0,
        email: None,
        flags: None,
        global_name: None,
        id: Id::new(id),
        locale: None,
        mfa_enabled: None,
        name: name.to_string(),
        premium_type: None,
        primary_guild: None,
        public_flags: None,
        system: None,
        verified: None,
    }
}

/// An empty rich embed.
pub(crate) fn embed() -> Embed {
    Embed {
        author: None,
        color: None,
        description: None,
        fields: Vec::new(),
        footer: None,
        image: None,
        kind: "rich".to_string(),
        provider: None,
        thumbnail: None,
        timestamp: None,
        title: None,
        url: None,
        video: None,
    }
}
