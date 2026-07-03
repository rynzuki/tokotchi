//! The full-screen pet card (crossterm). Hand-drawn to match the original curses look:
//! rounded border, ASCII creature with blink + bob, ambient sparkle particles confined
//! to the creature band, an ~8s level-up celebration, an xp bar, and the Σ total.
//!
//! Rendering is double-buffered: each frame is composed into a back buffer, then only the
//! cells that changed since the last frame are written to the terminal. This avoids the
//! whole-screen clear that made the card flicker.

use std::io::{self, IsTerminal, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};

use crate::ledger;
use crate::model::*;

const NEED_TTY: &str = "\
Tokotchi needs a real terminal — it can't run inside Claude Code's `!` prompt
or any captured shell.

Open a SEPARATE terminal window (Terminal, iTerm, Warp…) that is a normal shell,
and run:

    tokotchi
";

/// Shared with the background refresh thread.
struct Shared {
    total: u64,
    celebrate_until: f64,
}

/// A drifting sparkle: lives only in the creature band, twinkles over its short life.
struct Particle {
    row: i32,
    col: i32,
    ch: char,
    age: i32,
    ttl: i32,
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

    /// Write `text` at (row, col), one cell per char, clipped to the buffer width.
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

/// Restores the terminal on any exit path (including panics) — curses' wrapper() did
/// this automatically; crossterm does not, so a panicking draw would strand raw mode.
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
    if let Err(e) = run_ui() {
        eprintln!("Tokotchi couldn't start the terminal UI ({e}).\n{NEED_TTY}");
    }
}

fn run_ui() -> io::Result<()> {
    let _guard = TermGuard::enter()?;

    let shared = Arc::new(Mutex::new(Shared {
        total: ledger::total(&ledger::read_ledger()),
        celebrate_until: 0.0,
    }));
    spawn_refresh(Arc::clone(&shared));

    let mut out = io::stdout();
    let mut particles: Vec<Particle> = Vec::new();
    let mut frame: u64 = 0;

    let (mut w, mut h) = terminal::size()?;
    execute!(out, Clear(ClearType::All))?;
    let mut front = Buffer::new(w, h); // mirrors what's on screen (blank after the clear)
    let mut back = Buffer::new(w, h);

    loop {
        let (nw, nh) = terminal::size()?;
        if (nw, nh) != (w, h) {
            w = nw;
            h = nh;
            execute!(out, Clear(ClearType::All))?;
            front = Buffer::new(w, h);
            back = Buffer::new(w, h);
        }

        back.clear();
        compose(&mut back, &shared, &mut particles, frame);
        flush_diff(&mut out, &mut front, &back)?;
        frame = frame.wrapping_add(1);

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(k) => match k.code {
                    KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => break,
                    _ => {}
                },
                Event::Resize(..) => {}
                _ => {}
            }
        }
    }
    Ok(())
}

/// Emit only the cells that differ from the last frame — no whole-screen clear.
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

/// One immediate refresh, then a re-scan every AUTO_REFRESH_SECS. Threads are daemonic:
/// they die when the process exits after the TUI loop returns. A level increase across a
/// refresh opens the celebration window.
fn spawn_refresh(shared: Arc<Mutex<Shared>>) {
    let once = Arc::clone(&shared);
    thread::spawn(move || do_refresh(&once));
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(AUTO_REFRESH_SECS));
        do_refresh(&shared);
    });
}

fn do_refresh(shared: &Mutex<Shared>) {
    let new_total = ledger::total(&ledger::refresh());
    let mut s = shared.lock().unwrap();
    if level_for(new_total) > level_for(s.total) {
        s.celebrate_until = ledger::now_secs() + CELEBRATE_TUI_SECS;
    }
    s.total = new_total;
}

// ── particles ────────────────────────────────────────────────────────────────
fn step_particles(ps: &mut Vec<Particle>, band_rows: i32, iw: i32, celebrating: bool) {
    for p in ps.iter_mut() {
        p.age += 1;
    }
    ps.retain(|p| p.age < p.ttl);

    let (chance, cap) = if celebrating { (0.85, 26) } else { (0.22, 10) };
    if (ps.len() as i32) < cap && fastrand::f64() < chance {
        ps.push(Particle {
            row: fastrand::i32(0..=band_rows),
            col: fastrand::i32(0..iw),
            ch: SPARKLE_CHARS[fastrand::usize(0..SPARKLE_CHARS.len())],
            age: 0,
            ttl: fastrand::i32(10..=22),
        });
    }
}

// ── compose one frame into the back buffer ─────────────────────────────────────
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

fn compose(buf: &mut Buffer, shared: &Mutex<Shared>, particles: &mut Vec<Particle>, frame: u64) {
    let w = buf.w;
    let h = buf.h;

    let (total, celebrate_until) = {
        let s = shared.lock().unwrap();
        (s.total, s.celebrate_until)
    };
    let lvl = level_for(total);
    let stage = stage_for(lvl);
    let ckey = stage.color;
    let (frac, into, span) = progress(total);
    let celebrating = ledger::now_secs() < celebrate_until;

    // too small for the card → compact one-liner
    if h < PANEL_H + 1 || w < PANEL_W + 1 {
        let line = format!("Tokotchi · Lv {lvl} · Σ {}  (resize for full view)", humanize(total));
        buf.put(0, 0, &line, ACCENT, false, false);
        return;
    }

    let top = (h - PANEL_H) / 2;
    let left = (w - PANEL_W) / 2;
    let iw = PANEL_W - 2; // interior width

    // Center `text` on interior `row` (0-based, under the top border).
    macro_rules! panel {
        ($row:expr, $text:expr, $fg:expr, $bold:expr, $dim:expr) => {{
            let t: &str = &$text;
            let col = left + 1 + iw.saturating_sub(char_len(t)) / 2;
            buf.put(top + 1 + $row, col, t, $fg, $bold, $dim);
        }};
    }

    draw_box(buf, top, left, PANEL_H, PANEL_W, ACCENT);

    // title inset on the top border
    let title = " ❋ TOKOTCHI ❋ ";
    buf.put(top, left + (PANEL_W - char_len(title)) / 2, title, ACCENT, true, false);

    // sparkles first, so the creature always renders on top of them
    step_particles(particles, (CREATURE_TOP + CREATURE_BAND) as i32, iw as i32, celebrating);
    let spark_fg = if celebrating { SPARK } else { ckey };
    let mut chbuf = [0u8; 4];
    for p in particles.iter() {
        let phase = p.age as f64 / p.ttl as f64;
        let (bold, dim) = if (0.33..=0.66).contains(&phase) { (true, false) } else { (false, true) };
        buf.put(top + 1 + p.row as u16, left + 1 + p.col as u16, p.ch.encode_utf8(&mut chbuf), spark_fg, bold, dim);
    }

    // creature — the ONLY thing the bob moves, kept inside its reserved band
    let blink = (frame % 36) < 2;
    let art = if blink { &stage.frames[1] } else { &stage.frames[0] };
    let bob: u16 = if (frame / 7) % 2 == 0 { 1 } else { 0 };
    let glow = stage.name == "Elder" || celebrating;
    for (i, line) in art.iter().enumerate() {
        panel!(CREATURE_TOP + bob + i as u16, line, ckey, glow, false);
    }

    // status block — fixed rows, never affected by the bob
    let base = CREATURE_TOP + CREATURE_BAND + 1; // interior row 8
    if celebrating {
        let pulse_bold = (frame / 4) % 2 == 0;
        panel!(base, "✦ LEVEL UP! ✦", SPARK, pulse_bold, !pulse_bold);
    } else {
        panel!(base, stage.name, ckey, true, false);
    }
    panel!(base + 1, format!("Lv {lvl}"), CREAM, true, false);

    // xp bar, drawn as filled + empty tracks
    let bar_w = (iw - 6) as usize;
    let filled = (frac * bar_w as f64).round() as usize;
    let barrow = top + 1 + base + 3;
    let barcol = left + 1 + 3;
    buf.put(barrow, barcol, &"━".repeat(filled), BAR, true, false);
    buf.put(barrow, barcol + filled as u16, &"━".repeat(bar_w - filled), FAINT, false, false);
    panel!(
        base + 4,
        format!("{} / {} → Lv {}", humanize(into), humanize(span), lvl + 1),
        MUTED,
        false,
        false
    );

    panel!(base + 6, format!("Σ {}", humanize(total)), ckey, false, false);

    // footer inset on the bottom border (auto-refreshes every AUTO_REFRESH_SECS)
    let foot = " [q] quit ";
    buf.put(top + PANEL_H - 1, left + (PANEL_W - char_len(foot)) / 2, foot, MUTED, false, false);
}
