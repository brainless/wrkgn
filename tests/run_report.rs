mod common;

use std::time::Duration;

use wrkgn::run::status::{NON_TTY_INTERVAL, Snapshot, StatusLine};
use wrkgn::run::{
    self, Api, ErrorLog, MAX_UNIT_ATTEMPTS, Outcome, RunSummary, render_report, write_report,
};

use common::{FakeClock, fresh_store};

fn summary(outcome: Outcome, errors: &ErrorLog) -> RunSummary {
    RunSummary {
        run_id: 42,
        command: "wrkgn-monthly".into(),
        outcome,
        started_at: 1_700_000_000,
        finished_at: 1_700_000_464,
        errors: errors.groups_sorted().into_iter().cloned().collect(),
        units_done: vec!["2026-10".into()],
        items_collected: 12,
        items_processed: 9,
        items_approved: 9,
        next_run: "resume 2026-09".into(),
        ..RunSummary::default()
    }
}

#[test]
fn errors_group_by_signature_with_counts_ordering_and_full_samples() {
    let long = format!(
        "error sending request for url (https://hn.test/v0/item/1.json) {}",
        "x".repeat(1500)
    );
    let mut log = ErrorLog::default();
    log.record("collect", "transport", Some(10), &long);
    log.record(
        "extract",
        "validation",
        Some(7),
        "empty description: source 7",
    );
    log.record(
        "collect",
        "transport",
        Some(30),
        &long.replace("/1.json", "/30.json"),
    );
    log.record(
        "collect",
        "transport",
        Some(20),
        &long.replace("/1.json", "/20.json"),
    );
    log.record(
        "extract",
        "validation",
        Some(8),
        "empty description: source 8",
    );

    let groups = log.groups_sorted();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].count, 3);
    assert_eq!(
        (groups[0].first_item, groups[0].last_item),
        (Some(10), Some(20))
    );
    assert_eq!(groups[1].signature, "empty description: source #");

    let text = render_report(&summary(Outcome::Completed, &log));
    let first = text.find("### 1. 3 x").unwrap();
    let second = text.find("### 2. 2 x").unwrap();
    assert!(first < second);
    // The sample is the first message, untruncated.
    assert!(text.contains(&long));
    assert!(groups[0].sample.chars().count() > 1500);
    assert!(text.contains("- Stage: collect  Kind: transport"));
    assert!(text.contains("- First item: 10  Last item: 20"));
}

#[test]
fn report_has_every_section_even_when_empty() {
    let text = render_report(&summary(Outcome::Completed, &ErrorLog::default()));
    for heading in [
        "# wrkgn-monthly run 42: completed",
        "## Errors (0 groups)",
        "## Work done",
        "## What remains",
    ] {
        assert!(text.contains(heading), "{heading}\n{text}");
    }
    assert!(text.contains("- Export: None"));
    assert!(text.contains("- Pending units: None"));
    assert!(text.contains("- Failed units (retry next run): None"));
    assert!(text.contains("- Needs manual action (attempts exhausted): None"));
    assert!(text.contains("- Started: 2023-11-14T22:13:20Z  Finished: 2023-11-14T22:21:04Z"));
    assert!(!text.contains("Note: previous run"));
}

#[test]
fn report_lists_what_remains_and_the_killed_run_note() {
    let mut s = summary(Outcome::StoppedOnFailure, &ErrorLog::default());
    s.tripped_api = Some(Api::HnFirebase);
    s.pending_units = vec!["2026-09".into()];
    s.failed_units = vec![("2026-08".into(), 2)];
    s.exhausted_units = vec!["2026-07".into()];
    s.killed_runs = vec![41];
    s.took_over_pid = Some(9921);
    s.export = Some("0123456789abcdef".into());
    let text = render_report(&s);
    assert!(text.contains("3 consecutive HN Firebase API failures"));
    assert!(text.contains("- Exit code: 2"));
    assert!(text.contains(&format!("2026-08 (2/{MAX_UNIT_ATTEMPTS} attempts)")));
    assert!(text.contains("2026-07 (try `wrkgn collect --thread <id>`"));
    assert!(text.contains("previous run 41 was found killed (stale lock, PID 9921)"));
    assert!(text.contains("- Export: 0123456789abcdef"));
}

#[test]
fn report_contains_no_secrets_and_samples_with_backticks_stay_fenced() {
    let mut log = ErrorLog::default();
    log.record(
        "extract",
        "provider",
        Some(1),
        "401 for key sk-ant-api03-SECRETSECRET and Authorization: Bearer TOPSECRET123 ?api_key=HUNTER2 ```code```",
    );
    let mut out = Vec::new();
    write_report(&mut out, &summary(Outcome::Completed, &log)).unwrap();
    let text = String::from_utf8(out).unwrap();
    for secret in ["SECRETSECRET", "TOPSECRET123", "HUNTER2"] {
        assert!(!text.contains(secret), "{secret} leaked:\n{text}");
    }
    assert!(text.contains("<redacted>"));
    assert!(
        text.contains("````\n"),
        "fence must outgrow the sample's backticks"
    );
}

#[test]
fn limit_reason_names_the_condition_that_applied() {
    let mut s = summary(Outcome::LimitReached, &ErrorLog::default());
    s.limit.cap = Some((5, 5, "thread".into()));
    let text = render_report(&s);
    assert!(text.contains("- Outcome: limit_reached (thread cap reached (5/5))"));
    assert!(!text.contains("backfill remains"));

    s.limit.cap = None;
    s.limit.backfill_next = Some("2022-02".into());
    let text = render_report(&s);
    assert!(text.contains("(more backfill remains (next: 2022-02))"));

    s.limit.cap = Some((5, 5, "thread".into()));
    let text = render_report(&s);
    assert!(text.contains("(thread cap reached (5/5); more backfill remains (next: 2022-02))"));
    assert!(text.contains("- Exit code: 0\n"));
}

#[test]
fn nonzero_exit_code_is_explained_in_the_header() {
    let mut s = summary(Outcome::LimitReached, &ErrorLog::default());
    s.exhausted_units = vec![
        "2026-01".into(),
        "2026-02".into(),
        "2026-03".into(),
        "2026-04".into(),
    ];
    let text = render_report(&s);
    assert!(
        text.contains(
            "- Exit code: 1 (4 units need manual action, all from earlier runs; 0 errors this run)"
        ),
        "{text}"
    );

    s.failed_this_run = vec!["2026-04".into()];
    s.failed_units = vec![("2026-05".into(), 1)];
    let text = render_report(&s);
    assert!(text.contains(
        "- Exit code: 1 (4 units need manual action, 1 exhausted this run, 3 from earlier runs; 1 failed unit will be retried next run; 0 errors this run)"
    ), "{text}");

    s.failed_this_run = s.exhausted_units.clone();
    s.exhausted_units.truncate(1);
    s.failed_units.clear();
    let text = render_report(&s);
    assert!(
        text.contains(
            "- Exit code: 1 (1 unit needs manual action, all exhausted this run; 0 errors this run)"
        ),
        "{text}"
    );

    let mut log = ErrorLog::default();
    log.record("collect", "http", None, "status 404");
    let text = render_report(&summary(Outcome::Completed, &log));
    assert!(text.contains("- Exit code: 1 (1 error this run)"), "{text}");
    // Exit 0, 2 and 130 carry no explanation.
    assert!(
        !render_report(&summary(Outcome::Completed, &ErrorLog::default()))
            .contains("Exit code: 0 (")
    );
}

#[test]
fn exit_codes_follow_the_outcome() {
    assert_eq!(Outcome::Completed.exit_code(false), 0);
    assert_eq!(Outcome::LimitReached.exit_code(false), 0);
    assert_eq!(Outcome::Completed.exit_code(true), 1);
    assert_eq!(Outcome::LimitReached.exit_code(true), 1);
    assert_eq!(Outcome::StoppedOnFailure.exit_code(false), 2);
    assert_eq!(Outcome::StoppedOnFailure.exit_code(true), 2);
    assert_eq!(Outcome::Interrupted.exit_code(false), 130);
    assert_eq!(Outcome::Interrupted.exit_code(true), 130);

    let mut log = ErrorLog::default();
    assert_eq!(summary(Outcome::Completed, &log).exit_code(), 0);
    log.record("collect", "http", None, "status 404");
    assert_eq!(summary(Outcome::Completed, &log).exit_code(), 1);
    let mut failed_unit = summary(Outcome::LimitReached, &ErrorLog::default());
    failed_unit.exhausted_units = vec!["2026-01".into()];
    assert_eq!(failed_unit.exit_code(), 1);
}

#[test]
fn non_tty_status_emits_on_item_change_or_after_thirty_seconds() {
    let clock = FakeClock::new();
    let mut status = StatusLine::new(Vec::new(), false, &clock);
    let mut snapshot = Snapshot {
        command: "monthly".into(),
        unit: Some("2026-10".into()),
        item: Some(("thread", 123)),
        ..Snapshot::default()
    };
    status.update(&snapshot); // first line
    status.update(&snapshot); // unchanged, no time passed
    clock.advance(Duration::from_secs(5));
    status.update(&snapshot); // still unchanged
    snapshot.collected = 5;
    status.update(&snapshot); // item count changed
    clock.advance(NON_TTY_INTERVAL - Duration::from_secs(1));
    status.update(&snapshot); // 29s since the last line
    clock.advance(Duration::from_secs(1));
    status.update(&snapshot); // 30s: heartbeat
    snapshot.extract = Some((3, 10));
    status.update(&snapshot); // extraction progress alone does not count as an item
    let text = String::from_utf8(status.into_inner()).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert_eq!(
        lines[0],
        "monthly | 2026-10 | thread 123 | collected 0 processed 0 failed 0"
    );
    assert_eq!(
        lines[1],
        "monthly | 2026-10 | thread 123 | collected 5 processed 0 failed 0"
    );
    assert!(!text.contains('\r'));
}

#[test]
fn tty_status_rewrites_in_place_and_clears() {
    let clock = FakeClock::new();
    let mut status = StatusLine::new(Vec::new(), true, &clock);
    let mut snapshot = Snapshot {
        command: "show_hn".into(),
        extract: Some((3, 10)),
        ..Snapshot::default()
    };
    status.clear(); // nothing drawn yet
    status.update(&snapshot);
    snapshot.processed = 1;
    status.update(&snapshot);
    status.clear();
    status.clear();
    let text = String::from_utf8(status.into_inner()).unwrap();
    assert_eq!(text.matches("\r\x1b[2K").count(), 3);
    assert!(text.contains("show_hn | collected 0 processed 0 failed 0 | Extracting 3/10 groups"));
    assert!(text.ends_with("\r\x1b[2K"));
    assert!(!text.contains('\n'));
}

#[test]
fn interrupted_run_is_recorded_with_counts_and_grouped_errors() {
    let (_dir, db, mut store) = fresh_store();
    let session = run::begin(&mut store, &db, "monthly", 1_000).unwrap();
    assert!(!session.control.should_stop());
    session.control.interrupt();
    assert!(session.control.should_stop());

    let mut log = ErrorLog::default();
    let long = "y".repeat(2500);
    log.record(
        "collect",
        "transport",
        Some(5),
        &format!("timeout on item 5 {long}"),
    );
    log.record(
        "collect",
        "transport",
        Some(6),
        &format!("timeout on item 6 {long}"),
    );
    let mut s = summary(Outcome::Interrupted, &log);
    s.run_id = session.run_id;
    run::finish(&mut store, &session, Outcome::Interrupted, &s, &log, 1_050).unwrap();

    let stored = store.driver_run(session.run_id).unwrap().unwrap();
    assert_eq!(stored.status, "interrupted");
    assert_eq!(stored.finished_at, Some(1_050));
    assert_eq!(stored.counts.units_done, 1);
    assert_eq!(stored.counts.items_collected, 12);
    assert_eq!(stored.counts.failures, 2);
    let errors = store.run_errors(session.run_id).unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].count, 2);
    assert_eq!(
        errors[0].sample.chars().count(),
        run::errors::SAMPLE_MAX_CHARS
    );
    assert_eq!(s.exit_code(), 130);
    // The lock is released when the session is dropped.
    drop(session);
    assert!(run::lock::acquire(&db, "monthly").is_ok());
}
