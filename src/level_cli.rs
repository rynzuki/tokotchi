//! `tokotchi level` — the fast, READ-mostly CLI the statusline calls every render.
//!
//! Prints `<level>\t<stage>\t<emoji>\t<levelup>\t<mood>\t<hearts>\t<streak>\t<generation>\n`.
//! Fields 1–4 are byte-compatible with the pre-care version, so the statusline's `cut -f1/2/4`
//! is untouched; fields 5–8 are the care layer. The level/stage reflect the auto-detected
//! counting mode (subscription/API/raw — see mode.rs).
//!
//! FIELD OWNERSHIP (see state.rs): this process writes ONLY the active mode's economy
//! (last_seen_sigma, level, celebrate_until), last_activity, and the streak (+ the death
//! transition). It never touches happiness/interaction fields, so it can't clobber a feed.

use crate::model::{level_for_gen, stage_for, CELEBRATE_SECS};
use crate::{care, ledger, mode, state};

pub fn run() {
    let m = mode::detect();
    let total = ledger::total(&ledger::read_ledger(), m);
    let now = ledger::now_secs();

    let (st, eco) = state::update(now, total, m, |st, eco| {
        // token usage refills vitality (global) and advances this mode's Σ edge
        if total > eco.last_seen_sigma {
            st.last_activity = now;
            eco.last_seen_sigma = total;
        }
        // death → new generation (idempotent; update re-reads, so this is CAS-safe)
        care::maybe_reap(st, eco, total, now);
        // per-generation level + level-up flash
        let lvl = level_for_gen(total, eco.birth_sigma);
        if lvl > eco.level {
            eco.celebrate_until = now + CELEBRATE_SECS;
        }
        eco.level = lvl;
        // care streak (credited on day boundaries)
        care::credit_streak(st, now);
    });

    let lvl = eco.level;
    let stage = stage_for(lvl);
    let levelup = if now < eco.celebrate_until { 1 } else { 0 };
    let mood = care::mood(&st, now);
    let hearts = care::hearts(&st, now);
    print!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        lvl,
        stage.name,
        stage.emoji,
        levelup,
        mood.as_str(),
        hearts,
        st.streak_days,
        st.generation
    );
}
