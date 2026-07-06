//! Ledger scan tests.
//!
//! `scan_fixture` always runs: a deterministic synthetic transcript tree, no external
//! tools. `scan_matches_jq_on_real_data` is opt-in (set TOKOTCHI_PARITY=1) — it snapshots
//! ~/.claude/projects and asserts the Rust scan totals identically to token-ledger.sh's jq
//! pipeline over the same frozen bytes (proving byte-parity without the live-append race).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use tokotchi::ledger;
use tokotchi::mode::CountMode;

fn tmp(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("tokotchi_{}_{}", tag, std::process::id()))
}

#[test]
fn scan_fixture() {
    let root = tmp("fixture");
    let _ = fs::remove_dir_all(&root);
    let a = root.join("projectA");
    let b = root.join("projectB");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();

    // s1 spans two lines in one file; s1 also appears in another project (accumulates).
    fs::write(
        a.join("t.jsonl"),
        concat!(
            r#"{"sessionId":"s1","message":{"usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#, "\n",
            r#"{"sessionId":"s1","message":{"usage":{"input_tokens":1,"output_tokens":0,"cache_creation_input_tokens":2,"cache_read_input_tokens":3}}}"#, "\n",
            "not json — skipped\n",
            r#"{"sessionId":"s2","message":{"usage":{"input_tokens":100}}}"#, "\n",
        ),
    )
    .unwrap();
    fs::write(
        b.join("t.jsonl"),
        concat!(
            r#"{"sessionId":"s1","message":{"usage":{"cache_read_input_tokens":4}}}"#, "\n",
            r#"{"no":"session"}"#, "\n",
        ),
    )
    .unwrap();
    // a non-jsonl file must be ignored
    fs::write(a.join("notes.txt"), "ignore me").unwrap();

    let scan = ledger::scan_dir(&root);
    // Raw weight == the old flat four-field sum, so parity numbers are unchanged.
    assert_eq!(ledger::session_total(&scan, "s1", CountMode::Raw), 25); // (10+5) + (1+2+3) + 4
    assert_eq!(ledger::session_total(&scan, "s2", CountMode::Raw), 100);
    assert_eq!(ledger::total(&scan, CountMode::Raw), 125);
    // Subscription drops cache: s1 content = 10+5+1 = 16, s2 = 100 → 116
    assert_eq!(ledger::total(&scan, CountMode::Subscription), 116);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn scan_matches_jq_on_real_data() {
    if std::env::var("TOKOTCHI_PARITY").as_deref() != Ok("1") {
        eprintln!("skip: set TOKOTCHI_PARITY=1 to run the real-data jq parity check");
        return;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let projects = format!("{home}/.claude/projects");
    if !PathBuf::from(&projects).exists() {
        eprintln!("skip: no {projects}");
        return;
    }
    if !Command::new("jq").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("skip: jq not installed");
        return;
    }

    // Frozen snapshot so Rust and jq read identical bytes (no live-append race).
    let snap = tmp("snap");
    let _ = fs::remove_dir_all(&snap);
    fs::create_dir_all(&snap).unwrap();
    let snap_projects = snap.join("projects");
    assert!(Command::new("cp")
        .arg("-R")
        .arg(&projects)
        .arg(&snap_projects)
        .status()
        .unwrap()
        .success());

    const JQ: &str = r#"reduce inputs as $l ({};
      ($l.message.usage) as $u
      | if ($l.sessionId and $u) then
          .[$l.sessionId] += ( ($u.input_tokens // 0) + ($u.output_tokens // 0)
                             + ($u.cache_creation_input_tokens // 0) + ($u.cache_read_input_tokens // 0) )
        else . end) | [.[]] | add // 0"#;
    let sh = format!(
        "find {} -name '*.jsonl' -print0 | xargs -0 cat 2>/dev/null | jq -n -c '{JQ}'",
        snap_projects.display()
    );
    let out = Command::new("sh").arg("-c").arg(&sh).output().unwrap();
    let jq_total: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();

    // Raw weight over a fresh scan == the jq four-field sum (no legacy carryover here).
    let rust_total: u64 = ledger::total(&ledger::scan_dir(&snap_projects), CountMode::Raw);

    let _ = fs::remove_dir_all(&snap);
    assert_eq!(rust_total, jq_total, "Rust scan ({rust_total}) != jq scan ({jq_total})");
}
