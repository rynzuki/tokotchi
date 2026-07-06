//! Token-counting mode — how per-session usage `Comp`onents collapse into the single
//! Σ that drives the pet. Auto-detected from the user's Claude plan (`~/.claude.json`),
//! overridable via `TOKOTCHI_MODE`. Each mode is an independent level *lens* over the
//! same one pet (see state.rs `by_mode`): switching modes re-levels, it never forks a
//! second creature.
//!
//!   Raw          — legacy cache-inclusive sum (input+output+cache). Preserves the
//!                  pre-modes pet; never auto-selected, only override / safe fallback.
//!   Subscription — content tokens only (input+output). Matches the usage figure a
//!                  Pro/Max plan meters you on; cache is excluded.
//!   Api          — cache-discounted, input-equivalent cost: input + output
//!                  + 1.25·cache_creation + 0.1·cache_read (integer-exact ×5/4, /10).

use serde::{Deserialize, Serialize};

use crate::ledger::Comp;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[serde(rename_all = "lowercase")]
pub enum CountMode {
    Raw,
    Subscription,
    Api,
}

impl CountMode {
    /// Collapse one session's components into this mode's weighted token count.
    /// Saturating throughout — a runaway transcript can't panic the statusline.
    pub fn weight(&self, c: &Comp) -> u64 {
        let content = c.input.saturating_add(c.output);
        match self {
            CountMode::Raw => content
                .saturating_add(c.cache_creation)
                .saturating_add(c.cache_read)
                .saturating_add(c.legacy),
            CountMode::Subscription => content,
            // 1.25× cache-create, 0.1× cache-read — integer-exact, weighted per session.
            CountMode::Api => content
                .saturating_add(c.cache_creation.saturating_mul(5) / 4)
                .saturating_add(c.cache_read / 10),
        }
    }

    /// Short tag for the dev panel / diagnostics.
    pub fn label(&self) -> &'static str {
        match self {
            CountMode::Raw => "raw",
            CountMode::Subscription => "sub",
            CountMode::Api => "api",
        }
    }

    /// Dev-panel cycle order: raw → sub → api → raw.
    pub fn next(&self) -> CountMode {
        match self {
            CountMode::Raw => CountMode::Subscription,
            CountMode::Subscription => CountMode::Api,
            CountMode::Api => CountMode::Raw,
        }
    }
}

/// The `oauthAccount` fields we care about from `~/.claude.json`.
pub struct Account {
    pub organization_type: Option<String>, // e.g. "claude_max", "claude_pro"
    pub billing_type: Option<String>,       // e.g. "stripe_subscription"
}

/// Pure detection decision (unit-testable, no I/O). Resolution order, first match wins:
///   1. explicit `TOKOTCHI_MODE` (auto/raw/subscription/sub/api)
///   2. a non-empty API key ⇒ Api
///   3. the account's plan shape (`claude_*` org or stripe subscription ⇒ Subscription;
///      any other oauth account ⇒ Api)
///   4. no signal ⇒ Raw (safe legacy fallback — preserves the pre-modes pet)
pub fn decide(env_mode: Option<&str>, has_api_key: bool, account: Option<&Account>) -> CountMode {
    if let Some(m) = env_mode {
        match m.trim().to_ascii_lowercase().as_str() {
            "raw" => return CountMode::Raw,
            "subscription" | "sub" => return CountMode::Subscription,
            "api" => return CountMode::Api,
            _ => {} // "auto"/""/unknown → fall through to auto-detection
        }
    }
    if has_api_key {
        return CountMode::Api;
    }
    if let Some(a) = account {
        let subscription = a.organization_type.as_deref().is_some_and(|t| t.starts_with("claude_"))
            || a.billing_type.as_deref() == Some("stripe_subscription");
        return if subscription { CountMode::Subscription } else { CountMode::Api };
    }
    CountMode::Raw
}

/// Detect the active mode from the environment + `~/.claude.json`.
pub fn detect() -> CountMode {
    let env_mode = std::env::var("TOKOTCHI_MODE").ok();
    let has_api_key = std::env::var("ANTHROPIC_API_KEY").ok().is_some_and(|s| !s.is_empty());
    let account = read_account();
    decide(env_mode.as_deref(), has_api_key, account.as_ref())
}

/// Best-effort read of `~/.claude.json` → oauthAccount. Missing/corrupt ⇒ None.
fn read_account() -> Option<Account> {
    let path = crate::ledger::home().join(".claude.json");
    let s = std::fs::read_to_string(path).ok()?;

    #[derive(Deserialize)]
    struct Root {
        #[serde(rename = "oauthAccount")]
        oauth_account: Option<Acc>,
    }
    #[derive(Deserialize)]
    struct Acc {
        #[serde(rename = "organizationType")]
        organization_type: Option<String>,
        #[serde(rename = "billingType")]
        billing_type: Option<String>,
    }

    let root: Root = serde_json::from_str(&s).ok()?;
    let a = root.oauth_account?;
    Some(Account { organization_type: a.organization_type, billing_type: a.billing_type })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comp(i: u64, o: u64, cw: u64, cr: u64, legacy: u64) -> Comp {
        Comp { input: i, output: o, cache_creation: cw, cache_read: cr, legacy }
    }

    #[test]
    fn weights_match_the_billing_model() {
        let c = comp(100, 200, 1_000, 10_000, 0);
        assert_eq!(CountMode::Subscription.weight(&c), 300); // content only
        assert_eq!(CountMode::Raw.weight(&c), 300 + 1_000 + 10_000); // everything
        // 300 + 1000*5/4 + 10000/10 = 300 + 1250 + 1000
        assert_eq!(CountMode::Api.weight(&c), 300 + 1_250 + 1_000);
    }

    #[test]
    fn legacy_counts_only_in_raw() {
        let c = comp(0, 0, 0, 0, 500);
        assert_eq!(CountMode::Raw.weight(&c), 500);
        assert_eq!(CountMode::Subscription.weight(&c), 0);
        assert_eq!(CountMode::Api.weight(&c), 0);
    }

    #[test]
    fn env_override_wins_over_account() {
        let max = Account { organization_type: Some("claude_max".into()), billing_type: None };
        assert_eq!(decide(Some("api"), false, Some(&max)), CountMode::Api);
        assert_eq!(decide(Some("raw"), true, Some(&max)), CountMode::Raw);
        assert_eq!(decide(Some("sub"), false, None), CountMode::Subscription);
    }

    #[test]
    fn auto_detection_by_plan() {
        let max = Account { organization_type: Some("claude_max".into()), billing_type: Some("stripe_subscription".into()) };
        let pro = Account { organization_type: Some("claude_pro".into()), billing_type: None };
        let console = Account { organization_type: Some("business".into()), billing_type: Some("invoice".into()) };
        assert_eq!(decide(Some("auto"), false, Some(&max)), CountMode::Subscription);
        assert_eq!(decide(None, false, Some(&pro)), CountMode::Subscription);
        assert_eq!(decide(None, false, Some(&console)), CountMode::Api); // oauth but not a plan
        assert_eq!(decide(None, true, Some(&max)), CountMode::Api); // api key beats plan
        assert_eq!(decide(None, false, None), CountMode::Raw); // no signal → legacy
    }
}
