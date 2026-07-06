//! Persistent token ledger — scans Claude Code transcripts into a per-session tally
//! at ~/.claude/.token_ledger.json so the all-time total survives transcript pruning.
//!
//! Each session stores its four raw usage `Comp`onents; the single Σ the pet runs on is
//! derived at read time by a `CountMode` (see mode.rs), so the same ledger yields a
//! subscription/API/raw total without a rescan. The pre-modes ledger stored one flat
//! `u64` per session — those migrate into `Comp.legacy` (counted by Raw only; the
//! Subscription/API split of already-pruned sessions is unrecoverable and reads as 0).
//!
//! The old shell mirror (claude/token-ledger/token-ledger.sh) summed the four fields in
//! jq for the statusline; the statusline now calls `tokotchi refresh`/`tokotchi sigma`
//! instead, so this Rust ledger is the single source of truth — no formula to keep in sync.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::WalkDir;

use crate::mode::CountMode;

/// One session's raw usage, kept un-collapsed so any `CountMode` can weight it later.
#[derive(Serialize, Deserialize, Default, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(default)]
pub struct Comp {
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    /// Pre-modes flat total for sessions pruned before the schema upgrade — Raw-only.
    pub legacy: u64,
}

pub type Ledger = BTreeMap<String, Comp>; // sessionId -> components (BTreeMap = stable key order)

/// The user's home dir, cross-platform: `HOME` on macOS/Linux, `USERPROFILE` on Windows.
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

pub fn ledger_path() -> PathBuf {
    home().join(".claude").join(".token_ledger.json")
}

fn projects_dir() -> PathBuf {
    home().join(".claude").join("projects")
}

pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn read_ledger() -> Ledger {
    match fs::read_to_string(ledger_path()) {
        Ok(s) => parse_ledger(&s),
        Err(_) => Ledger::new(),
    }
}

/// Parse the ledger file, accepting both the component schema and the old flat
/// `{sid: number}` schema (flat values land in `Comp.legacy`).
fn parse_ledger(s: &str) -> Ledger {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Entry {
        Comp(Comp), // objects → components
        Flat(u64),  // bare numbers → legacy carryover
    }
    match serde_json::from_str::<BTreeMap<String, Entry>>(s) {
        Ok(m) => m
            .into_iter()
            .map(|(sid, e)| {
                let comp = match e {
                    Entry::Comp(c) => c,
                    Entry::Flat(n) => Comp { legacy: n, ..Default::default() },
                };
                (sid, comp)
            })
            .collect(),
        Err(_) => Ledger::new(),
    }
}

/// All-time Σ under `mode` (saturating sum of per-session weights).
pub fn total(ledger: &Ledger, mode: CountMode) -> u64 {
    ledger.values().fold(0u64, |acc, c| acc.saturating_add(mode.weight(c)))
}

/// One session's weighted Δ under `mode` (0 if unknown).
pub fn session_total(ledger: &Ledger, sid: &str, mode: CountMode) -> u64 {
    ledger.get(sid).map_or(0, |c| mode.weight(c))
}

/// Sum components per sessionId across every transcript currently on disk.
pub fn scan_projects() -> Ledger {
    scan_dir(&projects_dir())
}

/// Scan every `*.jsonl` under `dir`, accumulating components per session. Streams
/// line-by-line (transcripts can be tens of MB); malformed lines are skipped. Takes an
/// explicit dir so tests can point it at a fixture without touching HOME.
pub fn scan_dir(dir: &Path) -> Ledger {
    let mut scan = Ledger::new();
    for entry in WalkDir::new(dir).into_iter().filter_map(Result::ok) {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(file) = File::open(entry.path()) else { continue };
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { continue }; // non-UTF8 / read error
            add_line(&mut scan, &line);
        }
    }
    scan
}

/// Fold one transcript line into `scan` (extracted so it's unit-testable).
fn add_line(scan: &mut Ledger, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    let Some(sid) = v.get("sessionId").and_then(Value::as_str) else { return };
    let Some(usage) = v.get("message").and_then(|m| m.get("usage")) else { return };
    let get = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
    let e = scan.entry(sid.to_string()).or_default();
    e.input = e.input.saturating_add(get("input_tokens"));
    e.output = e.output.saturating_add(get("output_tokens"));
    e.cache_creation = e.cache_creation.saturating_add(get("cache_creation_input_tokens"));
    e.cache_read = e.cache_read.saturating_add(get("cache_read_input_tokens"));
}

/// Prune-proof merge: fresh scan components win for present sessions (superseding any
/// legacy carryover); sessions only in the old ledger (transcript pruned) are retained.
pub fn merge(mut old: Ledger, scan: Ledger) -> Ledger {
    for (sid, comp) in scan {
        old.insert(sid, comp);
    }
    old
}

/// Write via temp-file + atomic rename in the same dir, so a concurrent reader never
/// sees a half-written file. Two writers racing => last rename wins, self-healing on the
/// next refresh — intentionally no cross-process lock.
pub fn write_ledger_atomic(ledger: &Ledger) -> io::Result<()> {
    let path = ledger_path();
    let dir = path.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let tmp = dir.join(format!(".token_ledger.json.tmp.{}", std::process::id()));
    fs::write(&tmp, serde_json::to_string(ledger)?)?;
    fs::rename(&tmp, &path)
}

/// Scan + merge + persist; returns the merged ledger. Best-effort write.
pub fn refresh() -> Ledger {
    let merged = merge(read_ledger(), scan_projects());
    let _ = write_ledger_atomic(&merged);
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comp(i: u64, o: u64, cw: u64, cr: u64) -> Comp {
        Comp { input: i, output: o, cache_creation: cw, cache_read: cr, legacy: 0 }
    }

    #[test]
    fn accumulates_components_across_lines() {
        let mut scan = Ledger::new();
        add_line(&mut scan, r#"{"sessionId":"s1","message":{"usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":2,"cache_read_input_tokens":3}}}"#);
        add_line(&mut scan, r#"{"sessionId":"s1","message":{"usage":{"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#);
        assert_eq!(scan.get("s1"), Some(&comp(11, 6, 2, 3)));
        // Raw sums all four: (10+5+2+3)+(1+1) = 22
        assert_eq!(total(&scan, CountMode::Raw), 22);
        // Subscription = input+output only = 17
        assert_eq!(total(&scan, CountMode::Subscription), 17);
    }

    #[test]
    fn skips_lines_without_session_or_usage() {
        let mut scan = Ledger::new();
        add_line(&mut scan, r#"{"message":{"usage":{"input_tokens":10}}}"#); // no sessionId
        add_line(&mut scan, r#"{"sessionId":"s2"}"#); // no usage
        add_line(&mut scan, "not json at all");
        add_line(&mut scan, r#"{"sessionId":"s3","message":{"usage":{"input_tokens":7}}}"#);
        assert_eq!(scan.len(), 1);
        assert_eq!(scan.get("s3"), Some(&comp(7, 0, 0, 0)));
    }

    #[test]
    fn parses_flat_legacy_into_legacy_field() {
        let led = parse_ledger(r#"{"pruned":100,"live":{"input":3,"output":4}}"#);
        assert_eq!(led.get("pruned"), Some(&Comp { legacy: 100, ..Default::default() }));
        assert_eq!(led.get("live"), Some(&comp(3, 4, 0, 0)));
        // legacy counts only in Raw
        assert_eq!(total(&led, CountMode::Raw), 100 + 7);
        assert_eq!(total(&led, CountMode::Subscription), 7);
    }

    #[test]
    fn merge_prunes_nothing_and_scan_wins() {
        let mut old = Ledger::new();
        old.insert("pruned".into(), Comp { legacy: 100, ..Default::default() }); // transcript gone
        old.insert("live".into(), comp(1, 1, 0, 0)); // stale, scan should overwrite
        let mut scan = Ledger::new();
        scan.insert("live".into(), comp(50, 5, 0, 0));
        scan.insert("new".into(), comp(9, 0, 0, 0));
        let merged = merge(old, scan);
        assert_eq!(merged.get("pruned"), Some(&Comp { legacy: 100, ..Default::default() }));
        assert_eq!(merged.get("live"), Some(&comp(50, 5, 0, 0)));
        assert_eq!(merged.get("new"), Some(&comp(9, 0, 0, 0)));
        // Raw: 100 (legacy) + 55 (live) + 9 (new) = 164
        assert_eq!(total(&merged, CountMode::Raw), 164);
        assert_eq!(session_total(&merged, "live", CountMode::Subscription), 55);
    }
}
