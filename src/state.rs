//! Persistent pet state (~/.claude/.tokotchi_state.json), schema v2 — the care/mortality
//! layer's memory: generation, birth Σ, happiness, activity stamps, streak, graveyard.
//!
//! FIELD-OWNERSHIP INVARIANT (keeps the frequent statusline `level` process from clobbering
//! an interaction): every writer goes through `update`, which re-reads the freshest file and
//! mutates ONLY the fields that writer owns. `level_cli` owns last_activity/last_seen_sigma/
//! level/celebrate_until (+ the death transition). Interactions own happiness*/fed_until/
//! last_fed/last_pet/name. Vitality is DERIVED (never stored), so `level` never needs to
//! persist it and structurally can't overwrite a feed.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::level_for;

pub const STATE_VERSION: u32 = 2;
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

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
pub struct State {
    pub version: u32,
    pub generation: u32,
    pub born_at: f64,
    pub birth_sigma: u64, // all-time Σ at hatch; level = level_for_gen(Σ, birth_sigma)
    pub name: Option<String>,

    // happiness: anchor + decay stamp (interaction-owned)
    pub happiness: f64,
    pub happiness_at: f64,

    // vitality inputs (DERIVED on read; level-owned stamps)
    pub last_activity: f64,
    pub last_seen_sigma: u64,
    pub fed_until: f64, // feeding pushes this forward → vitality buffer

    pub last_fed: f64,
    pub last_pet: f64,

    pub streak_days: u32,
    pub last_streak_day: i64, // now/86400

    pub level: u64, // per-generation level (level-up flash detection)
    pub celebrate_until: f64,

    pub graveyard: Vec<Grave>,
}

pub fn state_path() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join(".claude")
        .join(".tokotchi_state.json")
}

/// A freshly grandfathered Generation-1 pet: birth_sigma=0 (so it keeps its all-time level),
/// born healthy and full-happiness right now. Used for absent/corrupt/v1 files.
fn grandfather(now: f64, sigma: u64, level: u64, celebrate_until: f64) -> State {
    State {
        version: STATE_VERSION,
        generation: 1,
        // a grandfathered pet is already established — start it just past the newborn window
        // so it doesn't read as "newborn"; only true rebirths (gen 2+) get the fresh-egg glow.
        born_at: now - (6.0 * 3600.0) - 1.0,
        birth_sigma: 0,
        name: None,
        happiness: 100.0,
        happiness_at: now,
        last_activity: now,
        last_seen_sigma: sigma,
        fed_until: 0.0,
        last_fed: 0.0,
        last_pet: 0.0,
        streak_days: 0,
        last_streak_day: (now / DAY) as i64,
        level,
        celebrate_until,
        graveyard: Vec::new(),
    }
}

/// Decide the state from raw file contents (pure — no I/O, so it's unit-testable).
/// Returns (state, migrated?) where `migrated` means "persist this once".
pub fn migrate_parsed(raw: Option<&str>, now: f64, sigma: u64) -> (State, bool) {
    match raw {
        None => (grandfather(now, sigma, level_for(sigma), 0.0), true),
        Some(s) => match serde_json::from_str::<State>(s) {
            Ok(st) if st.version >= STATE_VERSION => (st, false),
            // v1 `{level, celebrate_until}` (or any pre-v2 shape): grandfather, preserving level.
            Ok(old) => {
                let level = if old.level > 0 { old.level } else { level_for(sigma) };
                (grandfather(now, sigma, level, old.celebrate_until), true)
            }
            // corrupt → never nuke into a dead pet; grandfather a fresh living one.
            Err(_) => (grandfather(now, sigma, level_for(sigma), 0.0), true),
        },
    }
}

pub fn load_or_migrate(now: f64, sigma: u64) -> (State, bool) {
    migrate_parsed(fs::read_to_string(state_path()).ok().as_deref(), now, sigma)
}

pub fn save(st: &State) -> io::Result<()> {
    let path = state_path();
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    fs::write(&tmp, serde_json::to_string(st)?)?;
    fs::rename(&tmp, &path)
}

/// Re-read the freshest state, apply `f` (which must touch ONLY the caller's owned fields),
/// then atomic-write — but only if something actually changed (or a migration is pending).
/// Returns the resulting state for display.
pub fn update<F: FnOnce(&mut State)>(now: f64, sigma: u64, f: F) -> State {
    let (mut st, migrated) = load_or_migrate(now, sigma);
    let before = serde_json::to_string(&st).unwrap_or_default();
    f(&mut st);
    let after = serde_json::to_string(&st).unwrap_or_default();
    if migrated || before != after {
        let _ = save(&st);
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_v1_grandfathered() {
        let (st, migrated) = migrate_parsed(Some(r#"{"level":49,"celebrate_until":0.0}"#), 1000.0, 2_401_000_000);
        assert!(migrated);
        assert_eq!(st.version, 2);
        assert_eq!(st.generation, 1);
        assert_eq!(st.birth_sigma, 0); // ⇒ keeps Lv 49 via level_for_gen(Σ, 0)
        assert_eq!(st.level, 49);
        assert_eq!(st.happiness, 100.0); // born full, not starving
        assert_eq!(st.last_activity, 1000.0);
        assert_eq!(st.last_seen_sigma, 2_401_000_000);
        assert!(st.graveyard.is_empty());
    }

    #[test]
    fn absent_and_corrupt_grandfather_a_living_pet() {
        for raw in [None, Some("not json"), Some("{")] {
            let (st, migrated) = migrate_parsed(raw, 500.0, 4_000_000);
            assert!(migrated);
            assert_eq!(st.generation, 1);
            assert_eq!(st.birth_sigma, 0);
            assert_eq!(st.level, level_for(4_000_000)); // == 2
            assert_eq!(st.happiness, 100.0);
        }
    }

    #[test]
    fn v2_passthrough_no_migration() {
        let mut st = grandfather(0.0, 0, 1, 0.0);
        st.generation = 3;
        st.name = Some("Pixel".into());
        st.happiness = 42.0;
        let json = serde_json::to_string(&st).unwrap();
        let (loaded, migrated) = migrate_parsed(Some(&json), 9999.0, 12345);
        assert!(!migrated);
        assert_eq!(loaded.generation, 3);
        assert_eq!(loaded.name.as_deref(), Some("Pixel"));
        assert_eq!(loaded.happiness, 42.0);
    }
}
