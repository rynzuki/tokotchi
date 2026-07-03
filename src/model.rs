//! Pure pet model — no I/O, no terminal. Level, progress, evolution stages, palette.
//! Everything here is a pure function of the all-time token total, and unit-tested.

pub const UNIT: u64 = 1_000_000; // 1 level-unit = 1M tokens; level = floor(sqrt(total/UNIT))

// Statusline level-up flag window (seconds) and the live-TUI celebration window.
pub const CELEBRATE_SECS: f64 = 45.0;
pub const CELEBRATE_TUI_SECS: f64 = 8.0;
pub const AUTO_REFRESH_SECS: u64 = 20; // background ledger re-scan cadence

// Fixed card geometry — the pet lives in a dedicated terminal tab, so the card NEVER
// resizes. Creatures of any size bottom-anchor on a shared floor inside the creature box.
pub const PANEL_W: u16 = 40;
pub const PANEL_H: u16 = 22;
pub const CREATURE_TOP: u16 = 1; // interior row where the creature box starts
pub const BOX_W: u16 = 22; // max creature art width (validated in anim.rs)
pub const BOX_H: u16 = 8; // creature box height; art may be up to BOX_H-1 tall (leaves a bob row)

// UI chrome colors (256-color indices, identical to the old curses palette).
pub const ACCENT: u8 = 173; // clay — title + card border
pub const CREAM: u8 = 223; // level number
pub const MUTED: u8 = 244; // secondary text / footer
pub const FAINT: u8 = 239; // empty xp track
pub const BAR: u8 = 173; // filled xp
pub const SPARK: u8 = 230; // pale gold — celebration sparkles

// Ambient/celebration sparkle glyphs (twinkle over each particle's short life).
pub const SPARKLE_CHARS: [char; 5] = ['·', '✦', '✧', '⋆', '✳'];

pub fn level_for(total: u64) -> u64 {
    (total / UNIT).isqrt().max(1)
}

/// Per-generation level: only tokens accrued since this pet hatched count.
pub fn level_for_gen(current_sigma: u64, birth_sigma: u64) -> u64 {
    level_for(current_sigma.saturating_sub(birth_sigma))
}

/// (fraction 0..1, tokens_into_level, tokens_needed_for_level) for the current level.
pub fn progress(total: u64) -> (f64, u64, u64) {
    let lvl = level_for(total);
    let floor_t = lvl * lvl * UNIT;
    let next_t = (lvl + 1) * (lvl + 1) * UNIT;
    let span = next_t - floor_t;
    let into = total.saturating_sub(floor_t);
    let frac = if span > 0 { (into as f64 / span as f64).min(1.0) } else { 0.0 };
    (frac, into, span)
}

/// Humanize a raw token count -> 2.31B / 12.3M / 4.5K / 512.
pub fn humanize(n: u64) -> String {
    let f = n as f64;
    if f >= 1e9 {
        format!("{:.2}B", f / 1e9)
    } else if f >= 1e6 {
        format!("{:.1}M", f / 1e6)
    } else if f >= 1e3 {
        format!("{:.1}K", f / 1e3)
    } else {
        format!("{n}")
    }
}

/// Per-stage ambient sparkle flavor — its own glyphs + spawn rate give each evolution a
/// distinct feel (dust, bubbles, embers, stars…).
pub struct Particle {
    pub glyphs: &'static [char],
    pub rate: f64, // spawn chance per frame
}

/// One evolution stage — just metadata now; the art lives in `art/<art_dir>/*.txt`
/// (see anim.rs). Adding a stage = a new art folder + one entry here.
pub struct Stage {
    pub min_level: u64,
    pub name: &'static str,
    pub emoji: &'static str, // statusline only
    pub color: u8,           // primary 256-color index; stages ramp warmer as they grow
    pub accent: u8,          // secondary tone for flair
    pub particle: Particle,  // ambient sparkle flavor
    pub art_dir: &'static str,
}

// Evolution ramp — pale cream → tan → gold → clay → coral → bright clay.
pub static STAGES: [Stage; 6] = [
    Stage {
        min_level: 1,
        name: "Egg",
        emoji: "🥚",
        color: 223,
        accent: 179,
        particle: Particle { glyphs: &['·', '.'], rate: 0.10 },
        art_dir: "egg",
    },
    Stage {
        min_level: 5,
        name: "Blob",
        emoji: "👾",
        color: 180,
        accent: 179,
        particle: Particle { glyphs: &['°', '∘', '·'], rate: 0.16 },
        art_dir: "blob",
    },
    Stage {
        min_level: 15,
        name: "Sprout",
        emoji: "🌱",
        color: 179,
        accent: 108,
        particle: Particle { glyphs: &['·', ',', '˙'], rate: 0.18 },
        art_dir: "sprout",
    },
    Stage {
        min_level: 30,
        name: "Critter",
        emoji: "🐱",
        color: 173,
        accent: 209,
        particle: Particle { glyphs: &['·', '✦', '✧', '⋆'], rate: 0.22 },
        art_dir: "critter",
    },
    Stage {
        min_level: 60,
        name: "Beast",
        emoji: "🐉",
        color: 209,
        accent: 215,
        particle: Particle { glyphs: &['·', '˙', '*', '✦'], rate: 0.26 },
        art_dir: "beast",
    },
    Stage {
        min_level: 100,
        name: "Elder",
        emoji: "👑",
        color: 215,
        accent: 230,
        particle: Particle { glyphs: &['✦', '✧', '⋆', '✳', '·'], rate: 0.30 },
        art_dir: "elder",
    },
];

pub fn stage_for(level: u64) -> &'static Stage {
    let mut chosen = &STAGES[0];
    for st in STAGES.iter() {
        if level >= st.min_level {
            chosen = st;
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_curve() {
        assert_eq!(level_for(0), 1); // floor is 1, never 0
        assert_eq!(level_for(999_999), 1);
        assert_eq!(level_for(1_000_000), 1);
        assert_eq!(level_for(4_000_000), 2);
        assert_eq!(level_for(2_401_000_000), 49); // ~current real total
        assert_eq!(level_for(2_500_000_000), 50);
    }

    #[test]
    fn progress_bounds() {
        // exactly on a level floor -> 0 progress
        let (frac, into, _span) = progress(49 * 49 * UNIT);
        assert_eq!(into, 0);
        assert!(frac.abs() < 1e-9);
        // total below level-1 floor still clamps to 0, never underflows
        let (frac, into, span) = progress(0);
        assert_eq!(into, 0);
        assert_eq!(span, (4 - 1) * UNIT);
        assert!(frac.abs() < 1e-9);
    }

    #[test]
    fn humanize_units() {
        assert_eq!(humanize(512), "512");
        assert_eq!(humanize(4_500), "4.5K");
        assert_eq!(humanize(12_300_000), "12.3M");
        assert_eq!(humanize(2_310_000_000), "2.31B");
        assert_eq!(humanize(0), "0");
    }

    #[test]
    fn stages() {
        assert_eq!(stage_for(1).name, "Egg");
        assert_eq!(stage_for(4).name, "Egg");
        assert_eq!(stage_for(5).name, "Blob");
        assert_eq!(stage_for(49).name, "Critter");
        assert_eq!(stage_for(250).name, "Elder");
    }

    #[test]
    fn per_generation_level() {
        assert_eq!(level_for_gen(2_401_000_000, 0), 49); // grandfathered gen 1
        assert_eq!(level_for_gen(2_401_000_000, 2_400_000_000), 1); // reborn: ~1M since birth
        assert_eq!(level_for_gen(2_400_000_000, 2_500_000_000), 1); // birth after current → floor 1
    }
}
