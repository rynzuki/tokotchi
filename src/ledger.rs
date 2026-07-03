//! Persistent token ledger — scans Claude Code transcripts into a per-session tally
//! at ~/.claude/.token_ledger.json so the all-time total survives transcript pruning.
//!
//! IMPORTANT: the token-sum formula and prune-proof merge below are duplicated in the
//! shell tool `claude/token-ledger/token-ledger.sh` (which the statusline uses for its
//! Σ/Δ). If Claude Code's transcript schema or the summed fields change, update BOTH.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use walkdir::WalkDir;

pub type Ledger = BTreeMap<String, u64>; // sessionId -> total tokens (BTreeMap = stable key order)

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
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
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => Ledger::new(),
    }
}

pub fn total(ledger: &Ledger) -> u64 {
    ledger.values().sum()
}

/// Sum tokens per sessionId across every transcript currently on disk.
pub fn scan_projects() -> Ledger {
    scan_dir(&projects_dir())
}

/// Scan every `*.jsonl` under `dir`, summing tokens per session. Streams line-by-line
/// (transcripts can be tens of MB); malformed lines are skipped. Takes an explicit dir
/// so tests can point it at a fixture without touching HOME.
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
    let sum = get("input_tokens")
        .saturating_add(get("output_tokens"))
        .saturating_add(get("cache_creation_input_tokens"))
        .saturating_add(get("cache_read_input_tokens"));
    let e = scan.entry(sid.to_string()).or_insert(0);
    *e = e.saturating_add(sum); // accumulate across the session's lines (matches jq `reduce +=`)
}

/// Prune-proof merge: fresh scan counts win for present sessions; sessions only in the
/// old ledger (their transcript was pruned) are retained. Mirrors jq's `.[0] * .[1]`.
pub fn merge(mut old: Ledger, scan: Ledger) -> Ledger {
    for (sid, tokens) in scan {
        old.insert(sid, tokens);
    }
    old
}

/// Write via temp-file + atomic rename in the same dir, so a concurrent reader (or the
/// shell writer) never sees a half-written file. Two writers racing => last rename wins,
/// which self-heals on the next refresh — intentionally no cross-process lock.
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

    #[test]
    fn sums_four_usage_fields_and_accumulates() {
        let mut scan = Ledger::new();
        add_line(&mut scan, r#"{"sessionId":"s1","message":{"usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":2,"cache_read_input_tokens":3}}}"#);
        add_line(&mut scan, r#"{"sessionId":"s1","message":{"usage":{"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#);
        assert_eq!(scan.get("s1"), Some(&22)); // (10+5+2+3) + (1+1) = 22
    }

    #[test]
    fn skips_lines_without_session_or_usage() {
        let mut scan = Ledger::new();
        add_line(&mut scan, r#"{"message":{"usage":{"input_tokens":10}}}"#); // no sessionId
        add_line(&mut scan, r#"{"sessionId":"s2"}"#); // no usage
        add_line(&mut scan, "not json at all");
        add_line(&mut scan, r#"{"sessionId":"s3","message":{"usage":{"input_tokens":7}}}"#);
        assert_eq!(scan.len(), 1);
        assert_eq!(scan.get("s3"), Some(&7)); // missing fields default to 0
    }

    #[test]
    fn merge_prunes_nothing_and_scan_wins() {
        let mut old = Ledger::new();
        old.insert("pruned".into(), 100); // transcript gone, must be retained
        old.insert("live".into(), 40); // stale count, scan should overwrite
        let mut scan = Ledger::new();
        scan.insert("live".into(), 55);
        scan.insert("new".into(), 9);
        let merged = merge(old, scan);
        assert_eq!(merged.get("pruned"), Some(&100));
        assert_eq!(merged.get("live"), Some(&55));
        assert_eq!(merged.get("new"), Some(&9));
        assert_eq!(total(&merged), 164);
    }
}
