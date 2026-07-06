//! Tokotchi — a collectible terminal pet that levels off your Claude token usage.
//!
//! Self-contained single binary: it scans your Claude Code transcripts, maintains a
//! per-session token ledger (~/.claude/.token_ledger.json), and derives the pet's
//! level and evolution entirely from the all-time token total. View-only — it just
//! grows as you burn tokens.
//!
//!   tokotchi           launch the TUI (needs a real terminal, in its own window)
//!   tokotchi level     print "<level>\t<stage>\t<emoji>\t<levelup>…" for a statusline
//!   tokotchi refresh   rescan transcripts + persist the ledger (background statusline job)
//!   tokotchi sigma     print the all-time Σ under the active counting mode
//!   tokotchi sigma SID  print one session's Δ under the active counting mode

use tokotchi::{ledger, level_cli, mode, tui};

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("level") => level_cli::run(),
        Some("refresh") => {
            let _ = ledger::refresh();
        }
        Some("sigma") => {
            let m = mode::detect();
            let led = ledger::read_ledger();
            match args.next() {
                Some(sid) => println!("{}", ledger::session_total(&led, &sid, m)),
                None => println!("{}", ledger::total(&led, m)),
            }
        }
        None => tui::run(),
        Some(other) => {
            eprintln!(
                "tokotchi: unknown subcommand '{other}' (use `level`, `sigma [SID]`, `refresh`, or no args for the TUI)"
            );
            std::process::exit(2);
        }
    }
}
