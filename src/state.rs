//! Persistent pet state (~/.claude/.tokotchi_state.json), schema v3.
//!
//! ONE pet, many level lenses. Wall-clock identity and care — generation, name, born_at,
//! happiness, feeding, streak, graveyard — are GLOBAL: the creature lives, dies, gets fed,
//! and reincarnates exactly once regardless of counting mode. What differs per `CountMode`
//! is only the Σ-anchored bookkeeping, kept in a per-mode `Economy` (`by_mode`): birth Σ,
//! last-seen Σ, level, and the level-up flash. So switching mode re-levels the same pet
//! (Subscription/API/Raw each show their honest level) without forking a second creature,
//! and a drought kills it identically in every lens.
//!
//! FIELD-OWNERSHIP INVARIANT (keeps the frequent statusline `level` process from clobbering
//! an interaction): every writer goes through `update`, which re-reads the freshest file and
//! mutates ONLY the fields that writer owns. `level_cli`/`tui` own the active mode's
//! `Economy` + last_activity (+ the death transition). Interactions own the global happiness*/
//! fed_until/last_fed/last_pet/name. Vitality is DERIVED (never stored).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::mode::CountMode;
use crate::model::level_for;

pub const STATE_VERSION: u32 = 3;
const DAY: f64 = 86400.0;

/// A past pet, kept in the graveyard/album.
#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
pub struct Grave {
    pub name: Option<String>,
    pub generation: u32,
    pub stage: String,
    pub peak_level: u64,
    pub born_at: f64,
    pub died_at: f64,
}

/// Per-mode, Σ-anchored bookkeeping. One entry per `CountMode` the pet has been viewed in.
#[derive(Serialize, Deserialize, Default, Clone, PartialEq)]
#[serde(default)]
pub struct Economy {
    /// The global `generation` this economy was last (re)born for. If it lags the pet's
    /// current generation, the economy is stale (a death happened while a different mode
    /// was active) and gets re-grandfathered as a fresh egg on next activation.
    pub gen_stamp: u32,
    pub birth_sigma: u64,      // mode Σ at hatch; level = level_for(mode_Σ - birth_sigma)
    pub last_seen_sigma: u64,  // last observed mode Σ (activity edge detector)
    pub level: u64,            // per-generation level (level-up flash detection)
    pub celebrate_until: f64,
}

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
pub struct State {
    pub version: u32,
    // ── global identity (one pet) ──
    pub generation: u32,
    pub born_at: f64,
    pub name: Option<String>,

    // ── global care (mode-independent, wall-clock) ──
    pub happiness: f64,
    pub happiness_at: f64,
    pub last_activity: f64, // last time ANY token was generated (drives vitality)
    pub fed_until: f64,     // feeding pushes this forward → vitality buffer
    pub last_fed: f64,
    pub last_pet: f64,
    pub streak_days: u32,
    pub last_streak_day: i64, // now/86400
    pub graveyard: Vec<Grave>,

    // ── per-mode level lenses ──
    pub by_mode: BTreeMap<CountMode, Economy>,
}

pub fn state_path() -> PathBuf {
    crate::ledger::home().join(".claude").join(".tokotchi_state.json")
}

/// Grandfather one mode's economy from its current Σ. Gen 1 uses `birth_sigma = 0` so the
/// pet shows its HONEST all-time level in that mode (Subscription ≈ Lv 4, API ≈ Lv 18);
/// gen 2+ is a fresh post-death egg counting only from `sigma` onward.
fn grandfather_economy(sigma: u64, generation: u32) -> Economy {
    let gen = generation.max(1);
    let birth = if gen <= 1 { 0 } else { sigma };
    Economy {
        gen_stamp: gen,
        birth_sigma: birth,
        last_seen_sigma: sigma,
        level: level_for(sigma.saturating_sub(birth)),
        celebrate_until: 0.0,
    }
}

/// A freshly grandfathered Generation-1 pet: born healthy and full-happiness now, with the
/// active mode's economy grandfathered to its honest all-time level. Used for absent/corrupt
/// files. Other modes' economies are created lazily when first activated.
fn grandfather(now: f64, sigma: u64, mode: CountMode) -> State {
    let mut by_mode = BTreeMap::new();
    by_mode.insert(mode, grandfather_economy(sigma, 1));
    State {
        version: STATE_VERSION,
        generation: 1,
        // a grandfathered pet is already established — start it just past the newborn window
        // so it doesn't read as "newborn"; only true rebirths (gen 2+) get the fresh-egg glow.
        born_at: now - (6.0 * 3600.0) - 1.0,
        name: None,
        happiness: 100.0,
        happiness_at: now,
        last_activity: now,
        fed_until: 0.0,
        last_fed: 0.0,
        last_pet: 0.0,
        streak_days: 0,
        last_streak_day: (now / DAY) as i64,
        graveyard: Vec::new(),
        by_mode,
    }
}

/// The pre-v3 single-lineage state (flat Σ fields). Parsed only to lift a v1/v2 file into
/// v3: the old lineage becomes the `Raw` economy (preserving its level), care lifts to the
/// global slots, and Subscription/API grandfather lazily on first activation.
#[derive(Deserialize, Default)]
#[serde(default)]
struct LegacyState {
    version: u32,
    generation: u32,
    born_at: f64,
    birth_sigma: u64,
    name: Option<String>,
    happiness: f64,
    happiness_at: f64,
    last_activity: f64,
    last_seen_sigma: u64,
    fed_until: f64,
    last_fed: f64,
    last_pet: f64,
    streak_days: u32,
    last_streak_day: i64,
    level: u64,
    celebrate_until: f64,
    graveyard: Vec<Grave>,
}

fn from_legacy(old: LegacyState) -> State {
    let generation = old.generation.max(1);
    let mut by_mode = BTreeMap::new();
    by_mode.insert(
        CountMode::Raw,
        Economy {
            gen_stamp: generation,
            birth_sigma: old.birth_sigma,
            last_seen_sigma: old.last_seen_sigma,
            level: old.level.max(1),
            celebrate_until: old.celebrate_until,
        },
    );
    State {
        version: STATE_VERSION,
        generation,
        born_at: old.born_at,
        name: old.name,
        happiness: old.happiness,
        happiness_at: old.happiness_at,
        last_activity: old.last_activity,
        fed_until: old.fed_until,
        last_fed: old.last_fed,
        last_pet: old.last_pet,
        streak_days: old.streak_days,
        last_streak_day: old.last_streak_day,
        graveyard: old.graveyard,
        by_mode,
    }
}

/// Ensure the active mode's economy exists and matches the pet's current generation.
/// Returns whether it had to (re)create it (⇒ persist once).
fn ensure_mode(st: &mut State, sigma: u64, mode: CountMode) -> bool {
    let stale = st.by_mode.get(&mode).map_or(true, |e| e.gen_stamp != st.generation);
    if stale {
        st.by_mode.insert(mode, grandfather_economy(sigma, st.generation));
    }
    stale
}

/// Decide the state from raw file contents (pure — no I/O, so it's unit-testable).
/// Returns (state, migrated?) where `migrated` means "persist this once". The active
/// mode's economy is guaranteed present.
pub fn migrate_parsed(raw: Option<&str>, now: f64, sigma: u64, mode: CountMode) -> (State, bool) {
    let (mut st, mut migrated) = match raw {
        None => (grandfather(now, sigma, mode), true),
        Some(s) => match serde_json::from_str::<State>(s) {
            Ok(st) if st.version >= STATE_VERSION => (st, false),
            // v1/v2 single-lineage shape → lift the old lineage into the Raw economy.
            _ => match serde_json::from_str::<LegacyState>(s) {
                Ok(old) => (from_legacy(old), true),
                // corrupt → never nuke into a dead pet; grandfather a fresh living one.
                Err(_) => (grandfather(now, sigma, mode), true),
            },
        },
    };
    migrated |= ensure_mode(&mut st, sigma, mode);
    (st, migrated)
}

pub fn load_or_migrate(now: f64, sigma: u64, mode: CountMode) -> (State, bool) {
    migrate_parsed(fs::read_to_string(state_path()).ok().as_deref(), now, sigma, mode)
}

pub fn save(st: &State) -> io::Result<()> {
    let path = state_path();
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    fs::write(&tmp, serde_json::to_string(st)?)?;
    fs::rename(&tmp, &path)
}

/// Re-read the freshest state, apply `f` to the global state + the active mode's economy
/// (each writer must touch ONLY its owned fields), then atomic-write — but only if something
/// actually changed (or a migration is pending). Returns the resulting (state, active economy).
pub fn update<F: FnOnce(&mut State, &mut Economy)>(
    now: f64,
    sigma: u64,
    mode: CountMode,
    f: F,
) -> (State, Economy) {
    let (mut st, migrated) = load_or_migrate(now, sigma, mode);
    let before = serde_json::to_string(&st).unwrap_or_default();
    // pull the active economy out so `f` can borrow st and eco independently; reinsert after.
    let mut eco = st.by_mode.remove(&mode).unwrap_or_else(|| grandfather_economy(sigma, st.generation));
    f(&mut st, &mut eco);
    st.by_mode.insert(mode, eco.clone());
    let after = serde_json::to_string(&st).unwrap_or_default();
    if migrated || before != after {
        let _ = save(&st);
    }
    (st, eco)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A representative pre-modes v2 file (grandfathered gen-1 pet at Lv 52).
    const V2: &str = r#"{"version":2,"generation":1,"born_at":1783092042.0,"birth_sigma":0,
        "name":"Niclas","happiness":100.0,"happiness_at":1783341405.0,"last_activity":1783344581.0,
        "last_seen_sigma":2718174087,"fed_until":1783514205.0,"last_fed":1783341405.0,
        "last_pet":1783341401.0,"streak_days":1,"last_streak_day":20640,"level":52,
        "celebrate_until":0.0,"graveyard":[]}"#;

    #[test]
    fn migrates_v2_lineage_into_raw_economy() {
        let (st, migrated) = migrate_parsed(Some(V2), 1_000.0, 21_500_000, CountMode::Subscription);
        assert!(migrated);
        assert_eq!(st.version, 3);
        assert_eq!(st.generation, 1);
        assert_eq!(st.name.as_deref(), Some("Niclas"));
        assert_eq!(st.happiness, 100.0); // care lifted, not reset
        assert_eq!(st.streak_days, 1);
        // Raw economy preserves the old lineage → Lv 52 stays.
        let raw = st.by_mode.get(&CountMode::Raw).unwrap();
        assert_eq!(raw.birth_sigma, 0);
        assert_eq!(raw.level, 52);
        assert_eq!(raw.last_seen_sigma, 2718174087);
        // Active mode (Subscription) grandfathered to its HONEST all-time level (birth=0).
        let sub = st.by_mode.get(&CountMode::Subscription).unwrap();
        assert_eq!(sub.birth_sigma, 0);
        assert_eq!(sub.level, level_for(21_500_000)); // ~Lv 4
        assert_eq!(sub.gen_stamp, 1);
    }

    #[test]
    fn absent_and_corrupt_grandfather_a_living_pet() {
        for raw in [None, Some("not json"), Some("{")] {
            let (st, migrated) = migrate_parsed(raw, 500.0, 4_000_000, CountMode::Api);
            assert!(migrated);
            assert_eq!(st.generation, 1);
            assert_eq!(st.happiness, 100.0);
            let api = st.by_mode.get(&CountMode::Api).unwrap();
            assert_eq!(api.birth_sigma, 0);
            assert_eq!(api.level, level_for(4_000_000)); // == 2
        }
    }

    #[test]
    fn v3_passthrough_but_lazily_adds_missing_mode() {
        // Start from a Subscription-only v3 file, then activate API → API economy appears.
        let (st1, _) = migrate_parsed(None, 9_999.0, 21_500_000, CountMode::Subscription);
        let json = serde_json::to_string(&st1).unwrap();

        let (st2, migrated) = migrate_parsed(Some(&json), 9_999.0, 360_000_000, CountMode::Api);
        assert!(migrated); // API economy was created
        assert!(st2.by_mode.contains_key(&CountMode::Subscription));
        let api = st2.by_mode.get(&CountMode::Api).unwrap();
        assert_eq!(api.level, level_for(360_000_000)); // ~Lv 18-19, honest all-time

        // Re-loading with the same mode is a no-op (no spurious migration).
        let json2 = serde_json::to_string(&st2).unwrap();
        let (_st3, migrated2) = migrate_parsed(Some(&json2), 9_999.0, 360_000_000, CountMode::Api);
        assert!(!migrated2);
    }
}
