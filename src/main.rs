//! Tokotchi — a collectible terminal pet that levels off your Claude token usage.
//!
//! Self-contained single binary: it scans your Claude Code transcripts, maintains a
//! per-session token ledger (~/.claude/.token_ledger.json), and derives the pet's
//! level and evolution entirely from the all-time token total. View-only — it just
//! grows as you burn tokens.
//!
//!   tokotchi          launch the TUI (needs a real terminal, in its own window)
//!   tokotchi level    print "<level>\t<stage>\t<emoji>\t<levelup>" for a statusline

use tokotchi::{level_cli, tui};

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("level") => level_cli::run(),
        None => tui::run(),
        Some(other) => {
            eprintln!("tokotchi: unknown subcommand '{other}' (use `level`, or no args for the TUI)");
            std::process::exit(2);
        }
    }
}
