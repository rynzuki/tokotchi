//! Pure care state-machine (no I/O, like `model.rs`): vitality/happiness decay, mood,
//! interactions, the death→reincarnation transition, and the care streak. All functions take
//! an injected `now` (and `sigma` where relevant) so they're fully deterministic/unit-tested.
//!
//! Keep the token-sum-independent tuning here. These constants are deliberately FORGIVING:
//! a user generating tokens keeps vitality pinned at full; death needs a ~10-day drought.

use crate::model::stage_for;
use crate::state::{Grave, State};

const DAY: f64 = 86400.0;

// ── vitality (survival) ──
pub const GRACE_DAYS: f64 = 3.0; // no vitality decay for this many token-idle days
pub const DEATH_DAYS: f64 = 10.0; // vitality reaches 0 (death) at this many idle days
pub const FEED_BUFFER_DAYS: f64 = 2.0; // each feed buys this much life
pub const FEED_BUFFER_CAP_DAYS: f64 = 5.0; // …but the buffer can't stack past this

// ── happiness (bond) ──
pub const HAPPINESS_DECAY_PER_DAY: f64 = 15.0; // full→grumpy(<25) in ~5d, full→0 in ~6.7d
pub const FEED_HAP: f64 = 25.0;
pub const PET_HAP: f64 = 15.0;
pub const FEED_COOLDOWN: f64 = 30.0 * 60.0; // 30 min
pub const PET_COOLDOWN: f64 = 60.0; // 60 s (anti-mash)

pub const NEWBORN_SECS: f64 = 6.0 * 3600.0; // fresh-egg glow window

/// 0..=100. Full while token activity (or a feed buffer) is recent; ramps to 0 over the
/// drought window. Purely derived — never stored.
pub fn vitality(st: &State, now: f64) -> f64 {
    let anchor = st.last_activity.max(st.fed_until);
    let idle_days = (now - anchor) / DAY;
    if idle_days <= GRACE_DAYS {
        100.0
    } else if idle_days >= DEATH_DAYS {
        0.0
    } else {
        100.0 - (idle_days - GRACE_DAYS) / (DEATH_DAYS - GRACE_DAYS) * 100.0
    }
}

/// 0..=100. The stored anchor decayed to `now`.
pub fn happiness_now(st: &State, now: f64) -> f64 {
    (st.happiness - HAPPINESS_DECAY_PER_DAY * (now - st.happiness_at) / DAY).clamp(0.0, 100.0)
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Mood {
    Newborn,
    Happy,
    Content,
    Hungry,
    Grumpy,
    Sick,
}

impl Mood {
    pub fn as_str(self) -> &'static str {
        match self {
            Mood::Newborn => "newborn",
            Mood::Happy => "happy",
            Mood::Content => "content",
            Mood::Hungry => "hungry",
            Mood::Grumpy => "grumpy",
            Mood::Sick => "sick",
        }
    }
    /// Negative moods that warrant a gentle statusline nudge.
    pub fn is_needy(self) -> bool {
        matches!(self, Mood::Hungry | Mood::Grumpy | Mood::Sick)
    }
    /// Use the sad art frame?
    pub fn is_down(self) -> bool {
        matches!(self, Mood::Grumpy | Mood::Sick)
    }
}

pub fn mood(st: &State, now: f64) -> Mood {
    if now - st.born_at < NEWBORN_SECS {
        return Mood::Newborn;
    }
    let vit = vitality(st, now);
    let hap = happiness_now(st, now);
    if vit < 25.0 {
        Mood::Sick
    } else if vit < 60.0 {
        Mood::Hungry
    } else if hap < 25.0 {
        Mood::Grumpy
    } else if hap < 60.0 {
        Mood::Content
    } else {
        Mood::Happy
    }
}

/// Heart meter 0..=5 for the statusline / TUI.
pub fn hearts(st: &State, now: f64) -> u32 {
    (happiness_now(st, now) / 20.0).round() as u32
}

#[derive(Debug, PartialEq)]
pub enum CareResult {
    Done,
    Cooldown,
}

pub fn feed(st: &mut State, now: f64) -> CareResult {
    if now - st.last_fed < FEED_COOLDOWN {
        return CareResult::Cooldown;
    }
    st.happiness = (happiness_now(st, now) + FEED_HAP).min(100.0);
    st.happiness_at = now;
    // vitality buffer: extend fed_until, capped so it can't stack indefinitely
    let base = st.fed_until.max(now);
    st.fed_until = (base + FEED_BUFFER_DAYS * DAY).min(now + FEED_BUFFER_CAP_DAYS * DAY);
    st.last_fed = now;
    CareResult::Done
}

pub fn pet(st: &mut State, now: f64) -> CareResult {
    if now - st.last_pet < PET_COOLDOWN {
        return CareResult::Cooldown;
    }
    st.happiness = (happiness_now(st, now) + PET_HAP).min(100.0);
    st.happiness_at = now;
    st.last_pet = now;
    CareResult::Done
}

/// If the pet has died (vitality 0), record it in the graveyard and hatch a new generation
/// whose level counts only tokens from now on (`birth_sigma = sigma`). Returns whether a
/// death happened. Idempotent by construction: after this the pet is a fresh living gen, so a
/// second observer sees vitality > 0 and does nothing (see the CAS note in state::update).
pub fn maybe_reap(st: &mut State, sigma: u64, now: f64) -> bool {
    if vitality(st, now) > 0.0 {
        return false;
    }
    let anchor = st.last_activity.max(st.fed_until);
    let died_at = anchor + DEATH_DAYS * DAY; // the moment vitality actually hit 0
    st.graveyard.push(Grave {
        name: st.name.clone(),
        generation: st.generation,
        stage: stage_for(st.level).name.to_string(),
        peak_level: st.level, // per-gen level is monotonic, so current == peak
        born_at: st.born_at,
        died_at,
    });

    st.generation += 1;
    st.birth_sigma = sigma;
    st.born_at = now;
    st.name = None;
    st.happiness = 100.0;
    st.happiness_at = now;
    st.last_activity = now;
    st.last_seen_sigma = sigma;
    st.fed_until = 0.0;
    st.last_fed = 0.0;
    st.last_pet = 0.0;
    st.level = 1;
    st.celebrate_until = 0.0;
    st.streak_days = 0;
    st.last_streak_day = (now / DAY) as i64;
    true
}

/// Credit one day of care streak on a day boundary. "Good standing" = had activity today
/// (token use or an interaction) and not Sick. A missed bad day breaks the streak.
pub fn credit_streak(st: &mut State, now: f64) {
    let today = (now / DAY) as i64;
    if today <= st.last_streak_day {
        return; // already accounted for today
    }
    let day_of = |t: f64| (t / DAY) as i64;
    let active_today =
        day_of(st.last_activity) == today || day_of(st.last_fed) == today || day_of(st.last_pet) == today;
    let good = active_today && mood(st, now) != Mood::Sick;
    if good {
        st.streak_days = if today == st.last_streak_day + 1 { st.streak_days + 1 } else { 1 };
        st.last_streak_day = today;
    } else if today > st.last_streak_day + 1 {
        st.streak_days = 0; // a full bad day passed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::migrate_parsed;

    fn fresh(now: f64, sigma: u64) -> State {
        migrate_parsed(None, now, sigma).0
    }

    #[test]
    fn vitality_curve() {
        let now = 100.0 * DAY;
        let mut st = fresh(now, 1_000_000);
        // just active → full
        assert_eq!(vitality(&st, now), 100.0);
        // within grace → full
        st.last_activity = now - 3.0 * DAY;
        st.fed_until = 0.0;
        assert_eq!(vitality(&st, now), 100.0);
        // midway (6.5 idle days = grace+3.5 of 7-day ramp)
        st.last_activity = now - 6.5 * DAY;
        assert!((vitality(&st, now) - 50.0).abs() < 0.001);
        // past death window → 0
        st.last_activity = now - 11.0 * DAY;
        assert_eq!(vitality(&st, now), 0.0);
    }

    #[test]
    fn feed_adds_happiness_buffer_and_respects_cooldown() {
        let now = 50.0 * DAY;
        let mut st = fresh(now, 0);
        st.happiness = 40.0;
        st.happiness_at = now;
        assert_eq!(feed(&mut st, now), CareResult::Done);
        assert_eq!(st.happiness, 65.0);
        assert!(st.fed_until >= now + FEED_BUFFER_DAYS * DAY - 1.0);
        // immediate second feed → cooldown
        assert_eq!(feed(&mut st, now + 10.0), CareResult::Cooldown);
        // feed keeps a starving pet alive
        st.last_activity = now - 20.0 * DAY;
        assert!(vitality(&st, now) > 0.0, "feed buffer should hold vitality");
    }

    #[test]
    fn mood_thresholds() {
        let now = 100.0 * DAY;
        let mut st = fresh(now, 1_000_000);
        st.born_at = now - NEWBORN_SECS - 1.0; // past newborn glow
        st.happiness = 100.0;
        st.happiness_at = now;
        assert_eq!(mood(&st, now), Mood::Happy);
        st.happiness = 30.0;
        assert_eq!(mood(&st, now), Mood::Content);
        st.happiness = 10.0;
        assert_eq!(mood(&st, now), Mood::Grumpy);
        st.happiness = 100.0;
        st.last_activity = now - 6.5 * DAY; // vitality ~50 → hungry
        assert_eq!(mood(&st, now), Mood::Hungry);
        st.last_activity = now - 9.0 * DAY; // vitality ~14 → sick
        assert_eq!(mood(&st, now), Mood::Sick);
    }

    #[test]
    fn reap_starts_new_generation_from_death_sigma() {
        let now = 100.0 * DAY;
        let mut st = fresh(now, 2_400_000_000);
        st.name = Some("Critter".into());
        st.level = 49;
        st.last_activity = now - 11.0 * DAY; // dead
        st.fed_until = 0.0;

        assert!(maybe_reap(&mut st, 2_400_000_000, now));
        assert_eq!(st.generation, 2);
        assert_eq!(st.birth_sigma, 2_400_000_000); // gen 2 counts tokens from death onward
        assert_eq!(st.level, 1);
        assert!(st.name.is_none());
        assert_eq!(st.graveyard.len(), 1);
        assert_eq!(st.graveyard[0].name.as_deref(), Some("Critter"));
        assert_eq!(st.graveyard[0].peak_level, 49);
        assert_eq!(st.graveyard[0].generation, 1);
        // second reap is a no-op (fresh pet is alive)
        assert!(!maybe_reap(&mut st, 2_400_000_000, now));
        assert_eq!(st.graveyard.len(), 1);
    }
}
