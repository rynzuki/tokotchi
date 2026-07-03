//! `tokotchi level` — the fast, READ-ONLY CLI the statusline calls every render.
//!
//! Prints "<level>\t<stage>\t<emoji>\t<levelup 0|1>\n". Recomputes the level from the
//! ledger and flags a level-up for CELEBRATE_SECS after the stored level rises (tracked
//! in ~/.claude/.tokotchi_state.json). Never scans/writes the ledger — that's TUI-only.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ledger;
use crate::model::{level_for, stage_for, CELEBRATE_SECS};

fn state_path() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join(".claude")
        .join(".tokotchi_state.json")
}

#[derive(Serialize, Deserialize)]
struct State {
    level: u64,
    celebrate_until: f64,
}

fn read_state() -> Option<State> {
    serde_json::from_str(&fs::read_to_string(state_path()).ok()?).ok()
}

fn write_state(level: u64, celebrate_until: f64) {
    let path = state_path();
    let tmp = path.with_extension("json.tmp");
    let st = State { level, celebrate_until };
    if let Ok(json) = serde_json::to_string(&st) {
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

pub fn run() {
    let total = ledger::total(&ledger::read_ledger());
    let cur = level_for(total);
    let stage = stage_for(cur);
    let now = ledger::now_secs();

    let celeb = match read_state() {
        None => {
            // first ever call — record the level, never celebrate retroactively
            write_state(cur, 0.0);
            0.0
        }
        Some(st) => {
            if cur > st.level {
                let c = now + CELEBRATE_SECS;
                write_state(cur, c);
                c
            } else if cur < st.level {
                write_state(cur, 0.0);
                0.0
            } else {
                // unchanged level → no write; the celeb window (if any) keeps ticking down
                st.celebrate_until
            }
        }
    };

    let levelup = if now < celeb { 1 } else { 0 };
    print!("{}\t{}\t{}\t{}\n", cur, stage.name, stage.emoji, levelup);
}
