//! The full-screen pet card (crossterm). Hand-drawn, double-buffered (diff render, no
//! full-clear → no flicker). Shows the care layer — mood-driven face/tint, a heart meter,
//! streak, name, generation — and lets you feed/pet/name it and browse the graveyard.

use std::io::{self, IsTerminal, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};

use crate::anim::{self, ClipKind};
use crate::care::{self, CareResult, Mood};
use crate::model::*;
use crate::state::{self, State};
use crate::ledger;

const NEED_TTY: &str = "\
Tokotchi needs a real terminal — it can't run inside Claude Code's `!` prompt
or any captured shell.

Open a SEPARATE terminal window (Terminal, iTerm, Warp…) that is a normal shell,
and run:

    tokotchi
";

/// Dev-only live reload (see dev.sh): exit cleanly with this code when a src file changes.
const RELOAD_EXIT_CODE: i32 = 69;
const NAME_MAX: usize = 12;

struct Shared {
    total: u64,
    st: State,
}

struct Particle {
    row: i32,
    col: i32,
    ch: char,
    age: i32,
    ttl: i32,
}

enum Mode {
    Normal,
    Naming(String),
    Graveyard,
}

// ── screen buffer ─────────────────────────────────────────────────────────────
#[derive(Clone, PartialEq)]
struct Cell {
    ch: char,
    fg: u8,
    bold: bool,
    dim: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', fg: 0, bold: false, dim: false }
    }
}

struct Buffer {
    w: u16,
    h: u16,
    cells: Vec<Cell>,
}

impl Buffer {
    fn new(w: u16, h: u16) -> Self {
        Buffer { w, h, cells: vec![Cell::default(); w as usize * h as usize] }
    }
    fn clear(&mut self) {
        for c in &mut self.cells {
            *c = Cell::default();
        }
    }
    fn put(&mut self, row: u16, col: u16, text: &str, fg: u8, bold: bool, dim: bool) {
        if row >= self.h {
            return;
        }
        let mut c = col;
        for ch in text.chars() {
            if c >= self.w {
                break;
            }
            self.cells[row as usize * self.w as usize + c as usize] = Cell { ch, fg, bold, dim };
            c += 1;
        }
    }
}

struct TermGuard;

impl TermGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        Ok(TermGuard)
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

pub fn run() {
    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        print!("{NEED_TTY}");
        return;
    }
    match run_ui() {
        Ok(true) => std::process::exit(RELOAD_EXIT_CODE),
        Ok(false) => {}
        Err(e) => eprintln!("Tokotchi couldn't start the terminal UI ({e}).\n{NEED_TTY}"),
    }
}

fn latest_src_mtime() -> Option<std::time::SystemTime> {
    let mut latest: Option<std::time::SystemTime> = None;
    // watch both Rust source and the art tree so editing either triggers a dev reload
    for dir in ["src", "art"] {
        for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
            let ext = entry.path().extension().and_then(|e| e.to_str());
            if !matches!(ext, Some("rs") | Some("txt")) {
                continue;
            }
            if let Ok(md) = entry.metadata() {
                if let Ok(m) = md.modified() {
                    latest = Some(latest.map_or(m, |l: std::time::SystemTime| l.max(m)));
                }
            }
        }
    }
    latest
}

fn run_ui() -> io::Result<bool> {
    let _guard = TermGuard::enter()?;

    let now = ledger::now_secs();
    let total = ledger::total(&ledger::read_ledger());
    let st = state::load_or_migrate(now, total).0;
    let shared = Arc::new(Mutex::new(Shared { total, st }));
    spawn_refresh(Arc::clone(&shared));

    let mut out = io::stdout();
    let mut particles: Vec<Particle> = Vec::new();
    let mut frame: u64 = 0;
    let mut mode = Mode::Normal;
    let mut toast: Option<(String, f64)> = None;
    let mut happy_until = 0.0f64; // plays the Happy clip briefly after a feed/pet

    let (mut w, mut h) = terminal::size()?;
    execute!(out, Clear(ClearType::All))?;
    let mut front = Buffer::new(w, h);
    let mut back = Buffer::new(w, h);

    let dev = std::env::var_os("TOKOTCHI_DEV").is_some();
    let start_mtime = if dev { latest_src_mtime() } else { None };

    loop {
        let now = ledger::now_secs();
        if let Some((_, until)) = &toast {
            if now >= *until {
                toast = None;
            }
        }

        let (nw, nh) = terminal::size()?;
        if (nw, nh) != (w, h) {
            w = nw;
            h = nh;
            execute!(out, Clear(ClearType::All))?;
            front = Buffer::new(w, h);
            back = Buffer::new(w, h);
        }

        back.clear();
        compose(&mut back, &shared, &mut particles, frame, &mode, &toast, now < happy_until, now);
        flush_diff(&mut out, &mut front, &back)?;
        frame = frame.wrapping_add(1);

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(k) => {
                    if handle_key(k.code, &mut mode, &shared, &mut toast, &mut happy_until) {
                        return Ok(false); // quit
                    }
                }
                Event::Resize(..) => {}
                _ => {}
            }
        }

        if dev && frame % 5 == 0 {
            if let (Some(start), Some(cur)) = (start_mtime, latest_src_mtime()) {
                if cur > start {
                    return Ok(true);
                }
            }
        }
    }
}

/// Returns true if the app should quit.
fn handle_key(
    code: KeyCode,
    mode: &mut Mode,
    shared: &Mutex<Shared>,
    toast: &mut Option<(String, f64)>,
    happy_until: &mut f64,
) -> bool {
    let now = ledger::now_secs();
    match mode {
        Mode::Naming(buf) => match code {
            KeyCode::Enter => {
                let name = buf.trim().to_string();
                let total = shared.lock().unwrap().total;
                let st = state::update(now, total, |s| {
                    s.name = if name.is_empty() { None } else { Some(name.clone()) };
                });
                shared.lock().unwrap().st = st;
                *mode = Mode::Normal;
            }
            KeyCode::Esc => *mode = Mode::Normal,
            KeyCode::Backspace => {
                buf.pop();
            }
            KeyCode::Char(c) if !c.is_control() && buf.chars().count() < NAME_MAX => buf.push(c),
            _ => {}
        },
        Mode::Graveyard => match code {
            KeyCode::Char('g') | KeyCode::Char('q') | KeyCode::Esc => *mode = Mode::Normal,
            _ => {}
        },
        Mode::Normal => match code {
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => return true,
            KeyCode::Char('f') => interact(shared, toast, happy_until, now, care::feed, "*nom*  ♥", "already fed…"),
            KeyCode::Char('p') => interact(shared, toast, happy_until, now, care::pet, "♥", "still purring…"),
            KeyCode::Char('n') => {
                let cur = shared.lock().unwrap().st.name.clone().unwrap_or_default();
                *mode = Mode::Naming(cur);
            }
            KeyCode::Char('g') => *mode = Mode::Graveyard,
            _ => {}
        },
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn interact(
    shared: &Mutex<Shared>,
    toast: &mut Option<(String, f64)>,
    happy_until: &mut f64,
    now: f64,
    action: fn(&mut State, f64) -> CareResult,
    ok_msg: &str,
    cd_msg: &str,
) {
    let total = shared.lock().unwrap().total;
    let mut res = CareResult::Cooldown;
    let st = state::update(now, total, |s| res = action(s, now));
    shared.lock().unwrap().st = st;
    let done = res == CareResult::Done;
    if done {
        *happy_until = now + 1.6; // play the Happy clip while the reaction shows
    }
    *toast = Some(((if done { ok_msg } else { cd_msg }).to_string(), now + 1.6));
}

fn flush_diff(out: &mut impl Write, front: &mut Buffer, back: &Buffer) -> io::Result<()> {
    for row in 0..back.h {
        for col in 0..back.w {
            let i = row as usize * back.w as usize + col as usize;
            let b = &back.cells[i];
            if *b != front.cells[i] {
                queue!(out, MoveTo(col, row), SetForegroundColor(Color::AnsiValue(b.fg)))?;
                if b.bold {
                    queue!(out, SetAttribute(Attribute::Bold))?;
                }
                if b.dim {
                    queue!(out, SetAttribute(Attribute::Dim))?;
                }
                queue!(out, Print(b.ch), SetAttribute(Attribute::Reset), ResetColor)?;
                front.cells[i] = b.clone();
            }
        }
    }
    out.flush()
}

fn spawn_refresh(shared: Arc<Mutex<Shared>>) {
    let once = Arc::clone(&shared);
    thread::spawn(move || do_refresh(&once));
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(AUTO_REFRESH_SECS));
        do_refresh(&shared);
    });
}

fn do_refresh(shared: &Mutex<Shared>) {
    let now = ledger::now_secs();
    let total = ledger::total(&ledger::refresh());
    // same owned-field updates as `tokotchi level`, so the pet lives/grows while the TUI is open
    let st = state::update(now, total, |s| {
        if total > s.last_seen_sigma {
            s.last_activity = now;
            s.last_seen_sigma = total;
        }
        care::maybe_reap(s, total, now);
        let lvl = level_for_gen(total, s.birth_sigma);
        if lvl > s.level {
            s.celebrate_until = now + CELEBRATE_TUI_SECS;
        }
        s.level = lvl;
        care::credit_streak(s, now);
    });
    let mut sh = shared.lock().unwrap();
    sh.total = total;
    sh.st = st;
}

// ── particles ────────────────────────────────────────────────────────────────
fn step_particles(ps: &mut Vec<Particle>, band_rows: i32, iw: i32, glyphs: &[char], rate: f64, celebrating: bool) {
    for p in ps.iter_mut() {
        p.age += 1;
    }
    ps.retain(|p| p.age < p.ttl);
    // celebration overrides the stage flavor with a denser gold sparkle burst
    let (chance, cap, set): (f64, i32, &[char]) =
        if celebrating { (0.85, 26, &SPARKLE_CHARS) } else { (rate, 10, glyphs) };
    if (ps.len() as i32) < cap && fastrand::f64() < chance && !set.is_empty() {
        ps.push(Particle {
            row: fastrand::i32(0..=band_rows),
            col: fastrand::i32(0..iw),
            ch: set[fastrand::usize(0..set.len())],
            age: 0,
            ttl: fastrand::i32(10..=22),
        });
    }
}

// ── compose one frame ──────────────────────────────────────────────────────────
fn char_len(s: &str) -> u16 {
    s.chars().count() as u16
}

fn draw_box(buf: &mut Buffer, top: u16, left: u16, ph: u16, pw: u16, fg: u8) {
    let dashes = "─".repeat((pw - 2) as usize);
    buf.put(top, left, &format!("╭{dashes}╮"), fg, false, false);
    for r in 1..ph - 1 {
        buf.put(top + r, left, "│", fg, false, false);
        buf.put(top + r, left + pw - 1, "│", fg, false, false);
    }
    buf.put(top + ph - 1, left, &format!("╰{dashes}╯"), fg, false, false);
}

#[allow(clippy::too_many_arguments)]
fn compose(
    buf: &mut Buffer,
    shared: &Mutex<Shared>,
    particles: &mut Vec<Particle>,
    frame: u64,
    mode: &Mode,
    toast: &Option<(String, f64)>,
    happy: bool,
    now: f64,
) {
    let (w, h) = (buf.w, buf.h);
    let (total, st) = {
        let s = shared.lock().unwrap();
        (s.total, s.st.clone())
    };

    // too small for the card → compact one-liner
    if h < PANEL_H + 1 || w < PANEL_W + 1 {
        let line = format!("Tokotchi · Lv {} · Σ {}  (resize)", st.level, humanize(total.saturating_sub(st.birth_sigma)));
        buf.put(0, 0, &line, ACCENT, false, false);
        return;
    }

    let top = (h - PANEL_H) / 2;
    let left = (w - PANEL_W) / 2;
    let iw = PANEL_W - 2;

    macro_rules! panel {
        ($row:expr, $text:expr, $fg:expr, $bold:expr, $dim:expr) => {{
            let t: &str = &$text;
            let col = left + 1 + iw.saturating_sub(char_len(t)) / 2;
            buf.put(top + 1 + $row, col, t, $fg, $bold, $dim);
        }};
    }

    draw_box(buf, top, left, PANEL_H, PANEL_W, ACCENT);

    if let Mode::Graveyard = mode {
        compose_graveyard(buf, top, left, iw, &st);
        return;
    }

    let title = " ❋ TOKOTCHI ❋ ";
    buf.put(top, left + (PANEL_W - char_len(title)) / 2, title, ACCENT, true, false);

    let lvl = st.level;
    let stage = stage_for(lvl);
    let mood = care::mood(&st, now);
    let celebrating = now < st.celebrate_until;
    let blink = (frame % 36) < 2;
    let ckey = if mood == Mood::Sick { MUTED } else { stage.color };

    // ambient sparkles (per-stage flavor) behind the creature
    step_particles(
        particles,
        (CREATURE_TOP + BOX_H) as i32,
        iw as i32,
        stage.particle.glyphs,
        stage.particle.rate,
        celebrating,
    );
    let spark_fg = if celebrating { SPARK } else { stage.accent };
    let mut chbuf = [0u8; 4];
    for p in particles.iter() {
        let phase = p.age as f64 / p.ttl as f64;
        let (b, d) = if (0.33..=0.66).contains(&phase) { (true, false) } else { (false, true) };
        buf.put(top + 1 + p.row as u16, left + 1 + p.col as u16, p.ch.encode_utf8(&mut chbuf), spark_fg, b, d);
    }

    // creature — resolve the clip from mood/events, draw bottom-anchored + centered in the box
    let kind = if celebrating {
        ClipKind::Celebrate
    } else if happy {
        ClipKind::Happy
    } else if mood.is_down() {
        ClipKind::Sad
    } else if blink {
        ClipKind::Blink
    } else {
        ClipKind::Idle
    };
    let f = anim::frame_at(anim::sprite(stage.art_dir).clip(kind), frame);
    let bob: u16 = if (frame / 7) % 2 == 0 { 1 } else { 0 };
    let glow = (stage.name == "Elder" && mood != Mood::Sick) || celebrating;
    let art_top = CREATURE_TOP + BOX_H.saturating_sub(f.h).saturating_sub(bob); // grounded, hops on bob
    let art_left = left + 1 + iw.saturating_sub(f.w) / 2; // horizontally centered
    for (i, line) in f.lines.iter().enumerate() {
        buf.put(top + 1 + art_top + i as u16, art_left, line, ckey, glow, false);
    }

    let base = CREATURE_TOP + BOX_H + 1; // status block starts just below the box

    // title row: celebration / name / stage
    if celebrating {
        let pulse = (frame / 4) % 2 == 0;
        panel!(base, "✦ LEVEL UP! ✦", SPARK, pulse, !pulse);
    } else if let Mode::Naming(b) = mode {
        panel!(base, format!("name: {b}▏"), CREAM, true, false);
    } else if let Some(n) = &st.name {
        panel!(base, n.as_str(), ckey, true, false);
    } else {
        panel!(base, stage.name, ckey, true, false);
    }

    // level (+ stage when a name occupies the title row)
    let lvl_line = if st.name.is_some() { format!("Lv {lvl} · {}", stage.name) } else { format!("Lv {lvl}") };
    panel!(base + 1, lvl_line, CREAM, true, false);

    // hearts + mood, or a transient toast
    if let Some((msg, _)) = toast {
        panel!(base + 2, msg.as_str(), SPARK, true, false);
    } else {
        let hh = care::hearts(&st, now).min(5) as usize;
        let hearts = format!("{}{}  {}", "♥".repeat(hh), "♡".repeat(5 - hh), mood.as_str());
        panel!(base + 2, hearts, if mood.is_needy() { stage.color } else { MUTED }, false, mood == Mood::Sick);
    }

    // xp bar (per-generation)
    let (frac, into, span) = gen_progress(total, st.birth_sigma);
    let bar_w = (iw - 6) as usize;
    let filled = (frac * bar_w as f64).round() as usize;
    let barrow = top + 1 + base + 4;
    let barcol = left + 1 + 3;
    buf.put(barrow, barcol, &"━".repeat(filled), BAR, true, false);
    buf.put(barrow, barcol + filled as u16, &"━".repeat(bar_w - filled), FAINT, false, false);
    panel!(base + 5, format!("{} / {} → Lv {}", humanize(into), humanize(span), lvl + 1), MUTED, false, false);

    // generation + streak
    let mut meta = format!("Gen {}", st.generation);
    if st.streak_days > 0 {
        meta += &format!("   ·   {}d streak", st.streak_days);
    }
    panel!(base + 7, meta, MUTED, false, false);

    // per-generation Σ (the lifetime total is available in the statusline / wasted-on-claude)
    panel!(base + 8, format!("Σ {}", humanize(total.saturating_sub(st.birth_sigma))), ckey, false, false);

    // footer
    let foot = " [f]eed [p]et [n]ame [g]rave [q]uit ";
    buf.put(top + PANEL_H - 1, left + (PANEL_W - char_len(foot)) / 2, foot, MUTED, false, false);
}

/// Progress within the current generation's level.
fn gen_progress(total: u64, birth: u64) -> (f64, u64, u64) {
    progress(total.saturating_sub(birth))
}

fn compose_graveyard(buf: &mut Buffer, top: u16, left: u16, iw: u16, st: &State) {
    macro_rules! panel {
        ($row:expr, $text:expr, $fg:expr, $bold:expr) => {{
            let t: &str = &$text;
            let col = left + 1 + iw.saturating_sub(char_len(t)) / 2;
            buf.put(top + 1 + $row, col, t, $fg, $bold, false);
        }};
    }
    let title = " ❋ GRAVEYARD ❋ ";
    buf.put(top, left + (PANEL_W - char_len(title)) / 2, title, ACCENT, true, false);

    if st.graveyard.is_empty() {
        panel!(PANEL_H / 2 - 2, "no graves yet —", MUTED, false);
        panel!(PANEL_H / 2 - 1, "take good care of it.", MUTED, false);
    } else {
        let max_rows = (PANEL_H - 4) as usize;
        for (i, g) in st.graveyard.iter().rev().take(max_rows).enumerate() {
            let name = g.name.clone().unwrap_or_else(|| g.stage.clone());
            let line = format!("† Gen {}  {}  Lv {}", g.generation, name, g.peak_level);
            panel!(1 + i as u16, line, CREAM, false);
        }
    }
    let foot = " [g]/[q] back ";
    buf.put(top + PANEL_H - 1, left + (PANEL_W - char_len(foot)) / 2, foot, MUTED, false, false);
}
