//! Account risk signals read straight from the raw gateway JSON.
//!
//! twilight-model drops two signals the honeypot log embed and `/whois` rely on:
//!
//! - `unusual_dm_activity_until` has no field on any of twilight's member types
//!   (`MemberUpdate`, `PartialMember`, `InteractionMember`), so it is never
//!   deserialized.
//! - Discord's "likely spammer" mark (bit 20 of `public_flags`) has no
//!   `UserFlags` constant, and `UserFlags` deserializes with
//!   `from_bits_truncate`, so the bit is silently discarded.
//!
//! Both are moderator decision aids, so rather than lose them this module pulls
//! just those values out of the event's JSON text with minimal serde structs.
//! A REST re-fetch of the member was rejected: it would add a request (latency
//! and a failure point) in front of every ban. Parsing never fails the event: a
//! payload that doesn't match yields that signal's default (absent), and each
//! signal is read independently, so one malformed field never hides another.

use serde::Deserialize;
use std::collections::HashMap;
use twilight_model::id::{Id, marker::UserMarker};
use twilight_model::util::Timestamp;

/// Account risk signals twilight-model drops from the gateway payload.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AccountSignals {
    /// Discord has marked the account as a likely spammer.
    pub spammer: bool,
    /// When set to a future time, Discord has flagged the account for unusual
    /// DM activity.
    pub unusual_dm_activity_until: Option<Timestamp>,
}

/// Discord's "likely spammer" bit in a user's `public_flags`.
const SPAMMER_BIT: u64 = 1 << 20;

// Each signal is read through its own set of structs rather than one combined
// struct per event: serde fails a struct as a whole, so a single malformed field
// (e.g. a timestamp format twilight can't parse) would otherwise erase the other,
// unrelated signal too. The payload is deserialized once per signal, which is
// cheap next to the ban it informs.

#[derive(Deserialize)]
struct Envelope<T> {
    d: T,
}

/// A user object, read only for the spammer bit.
#[derive(Deserialize, Default)]
#[serde(default)]
struct RawUser {
    public_flags: Option<u64>,
}

impl RawUser {
    fn is_spammer(&self) -> bool {
        self.public_flags
            .is_some_and(|flags| flags & SPAMMER_BIT != 0)
    }
}

/// A member object, read only for the DM flag. `GUILD_MEMBER_UPDATE`'s payload
/// is itself a member object, so it is read through this struct directly.
#[derive(Deserialize, Default)]
#[serde(default)]
struct RawMember {
    unusual_dm_activity_until: Option<Timestamp>,
}

/// `GUILD_MEMBER_UPDATE`: the member's user, for the spammer bit.
#[derive(Deserialize, Default)]
#[serde(default)]
struct MemberUpdateUser {
    user: RawUser,
}

/// `MESSAGE_CREATE`: the author, for the spammer bit.
#[derive(Deserialize, Default)]
#[serde(default)]
struct MessageAuthor {
    author: RawUser,
}

/// `MESSAGE_CREATE`: the attached partial member, for the DM flag.
#[derive(Deserialize, Default)]
#[serde(default)]
struct MessageMember {
    member: Option<RawMember>,
}

/// `INTERACTION_CREATE`: the path down to the resolved data, generic over which
/// resolved map (`users` or `members`) is read.
#[derive(Deserialize)]
struct RawInteraction<T> {
    #[serde(default)]
    data: Option<RawInteractionData<T>>,
}

// Hand-written: `derive(Default)` would demand `T: Default`, which `None` doesn't need.
impl<T> Default for RawInteraction<T> {
    fn default() -> Self {
        Self { data: None }
    }
}

#[derive(Deserialize)]
struct RawInteractionData<T> {
    #[serde(default)]
    resolved: Option<T>,
}

impl<T> RawInteraction<T> {
    fn into_resolved(self) -> Option<T> {
        self.data.and_then(|data| data.resolved)
    }
}

/// Resolved users keyed by user id, for the spammer bit.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ResolvedUsers {
    users: HashMap<String, RawUser>,
}

/// Resolved members keyed by user id, for the DM flag.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ResolvedMembers {
    members: HashMap<String, RawMember>,
}

/// Deserializes the event's `d` payload, falling back to the default (signal
/// absent) when the JSON doesn't match.
fn parse<T: for<'de> Deserialize<'de> + Default>(raw: &str, event: &'static str) -> T {
    match serde_json::from_str::<Envelope<T>>(raw) {
        Ok(envelope) => envelope.d,
        Err(error) => {
            tracing::debug!(%error, event, "failed to read account signal from raw payload");
            T::default()
        }
    }
}

/// Reads the signals from a raw `GUILD_MEMBER_UPDATE` payload.
pub fn from_member_update(raw: &str) -> AccountSignals {
    const EVENT: &str = "GUILD_MEMBER_UPDATE";
    AccountSignals {
        spammer: parse::<MemberUpdateUser>(raw, EVENT).user.is_spammer(),
        unusual_dm_activity_until: parse::<RawMember>(raw, EVENT).unusual_dm_activity_until,
    }
}

/// Reads the signals from a raw `MESSAGE_CREATE` payload: the author's flags and
/// the attached partial member's DM flag.
pub fn from_message(raw: &str) -> AccountSignals {
    const EVENT: &str = "MESSAGE_CREATE";
    AccountSignals {
        spammer: parse::<MessageAuthor>(raw, EVENT).author.is_spammer(),
        unusual_dm_activity_until: parse::<MessageMember>(raw, EVENT)
            .member
            .and_then(|member| member.unusual_dm_activity_until),
    }
}

/// Reads the signals for `user_id` from the resolved data of a raw
/// `INTERACTION_CREATE` payload (e.g. a `/whois` user option).
pub fn from_resolved_user(raw: &str, user_id: Id<UserMarker>) -> AccountSignals {
    const EVENT: &str = "INTERACTION_CREATE";
    let key = user_id.to_string();
    AccountSignals {
        spammer: parse::<RawInteraction<ResolvedUsers>>(raw, EVENT)
            .into_resolved()
            .and_then(|resolved| resolved.users.get(&key).map(RawUser::is_spammer))
            .unwrap_or(false),
        unusual_dm_activity_until: parse::<RawInteraction<ResolvedMembers>>(raw, EVENT)
            .into_resolved()
            .and_then(|mut resolved| resolved.members.remove(&key))
            .and_then(|member| member.unusual_dm_activity_until),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DM_UNTIL: &str = "2030-01-01T00:00:00.000000+00:00";

    fn dm_until() -> Timestamp {
        Timestamp::parse(DM_UNTIL).expect("valid timestamp")
    }

    #[test]
    fn member_update_marks_spammer_when_bit_20_set() {
        let raw = format!(
            r#"{{"op":0,"t":"GUILD_MEMBER_UPDATE","d":{{"user":{{"id":"1","public_flags":{}}}}}}}"#,
            SPAMMER_BIT | 1
        );
        assert!(from_member_update(&raw).spammer);
    }

    #[test]
    fn member_update_without_bit_20_is_not_spammer() {
        let raw = r#"{"op":0,"d":{"user":{"id":"1","public_flags":4194304}}}"#;
        assert!(!from_member_update(raw).spammer);
    }

    #[test]
    fn member_update_reads_unusual_dm_activity_until() {
        let raw = format!(
            r#"{{"op":0,"d":{{"user":{{"id":"1"}},"unusual_dm_activity_until":"{DM_UNTIL}"}}}}"#
        );
        assert_eq!(
            from_member_update(&raw).unusual_dm_activity_until,
            Some(dm_until())
        );
    }

    #[test]
    fn message_reads_author_flags_and_member_dm_flag() {
        let raw = format!(
            r#"{{"op":0,"d":{{"author":{{"id":"1","public_flags":{SPAMMER_BIT}}},"member":{{"unusual_dm_activity_until":"{DM_UNTIL}"}}}}}}"#
        );
        assert_eq!(
            from_message(&raw),
            AccountSignals {
                spammer: true,
                unusual_dm_activity_until: Some(dm_until()),
            }
        );
    }

    #[test]
    fn resolved_user_looks_up_by_id_and_ignores_others() {
        let raw = format!(
            r#"{{"op":0,"d":{{"data":{{"resolved":{{
                "users":{{"1":{{"public_flags":0}},"2":{{"public_flags":{SPAMMER_BIT}}}}},
                "members":{{"1":{{}},"2":{{"unusual_dm_activity_until":"{DM_UNTIL}"}}}}
            }}}}}}}}"#
        );
        assert_eq!(
            from_resolved_user(&raw, Id::new(1)),
            AccountSignals::default()
        );
        assert_eq!(
            from_resolved_user(&raw, Id::new(2)),
            AccountSignals {
                spammer: true,
                unusual_dm_activity_until: Some(dm_until()),
            }
        );
    }

    #[test]
    fn malformed_dm_flag_does_not_hide_spammer_bit() {
        let bad_dm = r#""unusual_dm_activity_until":"not a timestamp""#;
        let member_update =
            format!(r#"{{"op":0,"d":{{"user":{{"public_flags":{SPAMMER_BIT}}},{bad_dm}}}}}"#);
        let message = format!(
            r#"{{"op":0,"d":{{"author":{{"public_flags":{SPAMMER_BIT}}},"member":{{{bad_dm}}}}}}}"#
        );
        let interaction = format!(
            r#"{{"op":0,"d":{{"data":{{"resolved":{{
                "users":{{"1":{{"public_flags":{SPAMMER_BIT}}}}},
                "members":{{"1":{{{bad_dm}}}}}
            }}}}}}}}"#
        );
        let expected = AccountSignals {
            spammer: true,
            unusual_dm_activity_until: None,
        };
        assert_eq!(from_member_update(&member_update), expected);
        assert_eq!(from_message(&message), expected);
        assert_eq!(from_resolved_user(&interaction, Id::new(1)), expected);
    }

    #[test]
    fn malformed_flags_do_not_hide_dm_flag() {
        let raw = format!(
            r#"{{"op":0,"d":{{"user":{{"public_flags":"oops"}},"unusual_dm_activity_until":"{DM_UNTIL}"}}}}"#
        );
        assert_eq!(
            from_member_update(&raw),
            AccountSignals {
                spammer: false,
                unusual_dm_activity_until: Some(dm_until()),
            }
        );
    }

    #[test]
    fn missing_fields_yield_defaults() {
        let raw = r#"{"op":0,"d":{}}"#;
        assert_eq!(from_member_update(raw), AccountSignals::default());
        assert_eq!(from_message(raw), AccountSignals::default());
        assert_eq!(
            from_resolved_user(raw, Id::new(1)),
            AccountSignals::default()
        );
    }

    #[test]
    fn malformed_json_yields_defaults() {
        let raw = "not json";
        assert_eq!(from_member_update(raw), AccountSignals::default());
        assert_eq!(from_message(raw), AccountSignals::default());
        assert_eq!(
            from_resolved_user(raw, Id::new(1)),
            AccountSignals::default()
        );
    }
}
