//! Reasons a realm character cannot be made portable (or exported) right now.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Blocker {
    /// `characters.online` is set: the realm holds newer state in memory than in the database. Phase 2 exports offline
    /// characters only; log the character out (or stop the realm) first. After a crash the flag can be stale.
    Online,
    /// The character is in the realm's "deleted characters" state.
    Deleted,
    /// Bots (`COABOT*`) and the Manager's own account are never portable.
    BotAccount,
    /// A challenge is in progress (`coa_character_challenge`).
    ActiveChallenge,
    /// A game mode such as hardcore is set (`coa_character_gamemode.gameMode <> 0`).
    ActiveGameMode,
    /// A custom trial is in progress (`coa_custom_trial_active`).
    ActiveCustomTrial,
    /// Manastorm caches are waiting for bag space (`ascension_manastorm_cache`): they name item guids that would be
    /// orphaned by the move.
    PendingManastormCaches,
    /// A table this Manager does not know holds rows of this character: exporting would silently leave them behind.
    UnclassifiedState(String),
}

impl Blocker {
    /// Stable identifier for the UI and for tests.
    pub fn code(&self) -> &'static str {
        match self {
            Blocker::Online => "online",
            Blocker::Deleted => "deleted",
            Blocker::BotAccount => "bot_account",
            Blocker::ActiveChallenge => "active_challenge",
            Blocker::ActiveGameMode => "active_game_mode",
            Blocker::ActiveCustomTrial => "active_custom_trial",
            Blocker::PendingManastormCaches => "pending_manastorm_caches",
            Blocker::UnclassifiedState(_) => "unclassified_state",
        }
    }

    /// Blockers that come from challenge / game mode / Manastorm state (decision D6).
    pub fn is_challenge_state(&self) -> bool {
        matches!(
            self,
            Blocker::ActiveChallenge
                | Blocker::ActiveGameMode
                | Blocker::ActiveCustomTrial
                | Blocker::PendingManastormCaches
        )
    }
}

impl fmt::Display for Blocker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Blocker::Online => {
                f.write_str("the character is online (or the realm did not log it out cleanly)")
            }
            Blocker::Deleted => f.write_str("the character is deleted"),
            Blocker::BotAccount => f.write_str("the character belongs to a bot or Manager account"),
            Blocker::ActiveChallenge => f.write_str("a challenge is in progress"),
            Blocker::ActiveGameMode => f.write_str("a game mode (such as hardcore) is active"),
            Blocker::ActiveCustomTrial => f.write_str("a custom trial is in progress"),
            Blocker::PendingManastormCaches => {
                f.write_str("Manastorm caches are waiting to be delivered")
            }
            Blocker::UnclassifiedState(table) => write!(
                f,
                "table `{table}` holds data of this character and is not known to this Manager"
            ),
        }
    }
}

/// Bot and Manager accounts, by the same rule the Manager already uses to hide them from the account list
/// (`accounts.rs`, `population.rs`).
pub fn is_internal_account(username: &str) -> bool {
    let upper = username.to_ascii_uppercase();
    upper.starts_with("COABOT") || upper == "COAMANAGER"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_accounts_follow_the_managers_own_rule() {
        for n in [
            "COABOTHOST1",
            "coabothost12",
            "COABOT",
            "COAMANAGER",
            "CoaManager",
        ] {
            assert!(is_internal_account(n), "{n}");
        }
        for n in ["ALICE", "MYCOABOT", "COAMANAGER2", "BOT1", ""] {
            assert!(!is_internal_account(n), "{n}");
        }
    }

    #[test]
    fn challenge_state_blockers_are_flagged_for_decision_d6() {
        assert!(Blocker::ActiveChallenge.is_challenge_state());
        assert!(Blocker::ActiveGameMode.is_challenge_state());
        assert!(Blocker::PendingManastormCaches.is_challenge_state());
        assert!(!Blocker::Online.is_challenge_state());
        assert!(!Blocker::UnclassifiedState("x".into()).is_challenge_state());
    }
}
