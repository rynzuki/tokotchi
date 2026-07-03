//! `tokotchi level` — the fast, READ-mostly CLI the statusline calls every render.
//!
//! Prints `<level>\t<stage>\t<emoji>\t<levelup>\t<mood>\t<hearts>\t<streak>\t<generation>\n`.
//! Fields 1–4 are byte-compatible with the pre-care version, so the statusline's `cut -f1/2/4`
//! is untouched; fields 5–8 are the care layer.
//!
//! FIELD OWNERSHIP (see state.rs): this process writes ONLY last_activity, last_seen_sigma,
//! level, celebrate_until (+ the death transition). It never touches happiness/interaction
//! fields, so it can't clobber a feed done in the TUI.

use crate::model::{level_for_gen, stage_for, CELEBRATE_SECS};
use crate::{care, ledger, state};

pub fn run() {
    let total = ledger::total(&ledger::read_ledger());
    let now = ledger::now_secs();

    let st = state::update(now, total, |st| {
        // token usage refills vitality
        if total > st.last_seen_sigma {
            st.last_activity = now;
            st.last_seen_sigma = total;
        }
        // death → new generation (idempotent; update re-reads, so this is CAS-safe)
        care::maybe_reap(st, total, now);
        // per-generation level + level-up flash
        let lvl = level_for_gen(total, st.birth_sigma);
        if lvl > st.level {
            st.celebrate_until = now + CELEBRATE_SECS;
        }
        st.level = lvl;
        // care streak (credited on day boundaries)
        care::credit_streak(st, now);
    });

    let lvl = st.level;
    let stage = stage_for(lvl);
    let levelup = if now < st.celebrate_until { 1 } else { 0 };
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
