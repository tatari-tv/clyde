#![allow(clippy::unwrap_used)]

use super::*;
use chrono::DateTime;
use session::ParsedSession;
use std::path::PathBuf;

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const UUID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

fn parsed(session_id: &str) -> ParsedSession {
    ParsedSession {
        session_id: session_id.to_string(),
        cwd: Some(PathBuf::from("/home/saidler/repos/tatari-tv/marquee")),
        project_dir: PathBuf::from("/home/saidler/.claude/projects/-home-saidler-repos-tatari-tv-marquee"),
        ai_title: Some("test session".into()),
        first_prompt: None,
        command_name: None,
        git_branch: None,
        model: None,
        n_msgs: 1,
        created: None,
        activity_at: None,
        modified: dt("2026-06-21T10:00:00Z"),
        body: "body".into(),
        jsonl_paths: vec![PathBuf::from("/tmp/does-not-exist.jsonl")],
    }
}

fn seed(db: &Db, session_id: &str) {
    db.upsert_session(&parsed(session_id), "desk").unwrap();
}

fn now() -> DateTime<Utc> {
    dt("2026-07-31T12:00:00Z")
}

// ---------------------------------------------------------------------------------------------
// The probe record: what may be written, and what must never be.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_conclusive_negative_is_recorded_with_its_outcome_and_time() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    assert!(db.record_probe(UUID_A, &ProbeOutcome::NoOrigin, now()).unwrap());
    assert_eq!(
        db.probe_of(UUID_A).unwrap().as_deref(),
        Some("no-origin@2026-07-31T12:00:00+00:00")
    );
}

/// The panel's severest finding, at the write. A transient environment failure must record NOTHING,
/// or one `safe.directory` error becomes a permanent refusal of work scope with no path back.
///
/// The guard lives at the write rather than at each call site precisely so a future caller cannot
/// reintroduce it by forgetting the check.
///
/// BITES: drop the `is_conclusive_negative` guard from `record_probe` and every row here stamps.
#[test]
fn a_transient_git_failure_never_stamps() {
    let db = Db::open_memory().unwrap();
    for outcome in [
        ProbeOutcome::Indeterminate,
        ProbeOutcome::Blocked,
        ProbeOutcome::OutsideRoot,
        ProbeOutcome::Resolved {
            slug: "tatari-tv/philo".into(),
            host: "github.com".into(),
        },
    ] {
        seed(&db, UUID_A);
        assert!(
            !db.record_probe(UUID_A, &outcome, now()).unwrap(),
            "{} must record nothing",
            outcome.as_str()
        );
        assert_eq!(
            db.probe_of(UUID_A).unwrap(),
            None,
            "{} left a stamp, which is a permanent lockout",
            outcome.as_str()
        );
    }
}

/// `NotARepo` is the OTHER conclusive arm. Both are recorded, and nothing else is.
#[test]
fn not_a_repo_is_conclusive_too() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    assert!(db.record_probe(UUID_A, &ProbeOutcome::NotARepo, now()).unwrap());
    assert!(db.probe_of(UUID_A).unwrap().unwrap().starts_with("not-a-repo@"));
}

/// The first stamp is the one that counts, and a later pass must not rewrite it. Two reasons: the
/// FIRST failed observation is the evidence, and an unconditional UPDATE on a column touched for
/// every session on every reindex pass would fire the v5 revision trigger forever.
#[test]
fn a_second_conclusive_probe_does_not_rewrite_the_first_stamp() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    db.record_probe(UUID_A, &ProbeOutcome::NoOrigin, now()).unwrap();
    let first = db.probe_of(UUID_A).unwrap();

    let later = dt("2026-08-05T09:00:00Z");
    assert!(
        !db.record_probe(UUID_A, &ProbeOutcome::NotARepo, later).unwrap(),
        "a row that already carries a stamp is not rewritten"
    );
    assert_eq!(db.probe_of(UUID_A).unwrap(), first);
}

/// The recovery path. `--clear-probe --session <id>` is NARROW by design: it names sessions, and it
/// never touches the rest of the catalog.
#[test]
fn clear_probe_clears_only_the_named_sessions() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    seed(&db, UUID_B);
    db.record_probe(UUID_A, &ProbeOutcome::NoOrigin, now()).unwrap();
    db.record_probe(UUID_B, &ProbeOutcome::NoOrigin, now()).unwrap();

    assert_eq!(db.clear_probe(&[UUID_A.to_string()]).unwrap(), 1);
    assert_eq!(db.probe_of(UUID_A).unwrap(), None);
    assert!(
        db.probe_of(UUID_B).unwrap().is_some(),
        "an unnamed session keeps its record"
    );
}

/// A cleared row re-stamps on the next pass if the cwd still declines conclusively. That is what
/// makes `--clear-probe` safe to hand an operator: it does not disable the gate, it re-asks.
#[test]
fn a_cleared_probe_restamps_on_the_next_conclusive_pass() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    db.record_probe(UUID_A, &ProbeOutcome::NoOrigin, now()).unwrap();
    db.clear_probe(&[UUID_A.to_string()]).unwrap();

    let later = dt("2026-08-05T09:00:00Z");
    assert!(db.record_probe(UUID_A, &ProbeOutcome::NoOrigin, later).unwrap());
    assert_eq!(
        db.probe_of(UUID_A).unwrap().as_deref(),
        Some("no-origin@2026-08-05T09:00:00+00:00")
    );
}

// ---------------------------------------------------------------------------------------------
// The operator override, and its audit trail.
// ---------------------------------------------------------------------------------------------

#[test]
fn an_override_stores_its_reason_actor_and_time() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    assert!(
        db.set_scope_override(UUID_A, OVERRIDE_WORK, "fork of a work repo", "saidler@desk", now())
            .unwrap()
    );

    let rows = db.scope_overrides().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].session_id, UUID_A);
    assert_eq!(rows[0].scope, OVERRIDE_WORK);
    assert_eq!(rows[0].reason, "fork of a work repo");
    assert_eq!(
        rows[0].by.as_deref(),
        Some("saidler@desk"),
        "the actor is $USER@host, not a bare username: catalogs get merged across machines"
    );
    assert_eq!(rows[0].at.as_deref(), Some("2026-07-31T12:00:00+00:00"));
}

/// An override with no recorded reason is a hole, not a hatch. Rejected at the write, not only at
/// the CLI, so a second caller cannot get in without one.
///
/// BITES: delete the `reason.trim().is_empty()` check and both of these succeed.
#[test]
fn an_override_without_a_reason_is_refused() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    assert!(
        db.set_scope_override(UUID_A, OVERRIDE_WORK, "", "saidler@desk", now())
            .is_err()
    );
    assert!(
        db.set_scope_override(UUID_A, OVERRIDE_WORK, "   ", "saidler@desk", now())
            .is_err()
    );
    assert!(db.scope_overrides().unwrap().is_empty());
}

/// The vocabulary is exactly two tokens. Anything else is refused loudly rather than stored and
/// silently read as `personal` later.
#[test]
fn an_override_rejects_a_scope_outside_the_vocabulary() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    assert!(
        db.set_scope_override(UUID_A, "Work", "capitalized", "saidler@desk", now())
            .is_err()
    );
    assert!(
        db.set_scope_override(UUID_A, "whatever", "nonsense", "saidler@desk", now())
            .is_err()
    );
}

#[test]
fn clearing_an_override_removes_its_whole_audit_trail() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    db.set_scope_override(UUID_A, OVERRIDE_PERSONAL, "misfiled clone", "saidler@desk", now())
        .unwrap();
    assert!(db.clear_scope_override(UUID_A).unwrap());
    assert!(db.scope_overrides().unwrap().is_empty());

    let row: (Option<String>, Option<String>, Option<String>) = db
        .conn
        .query_row(
            "SELECT scope_override_reason, scope_override_by, scope_override_at FROM sessions \
             WHERE session_id = ?1",
            params![UUID_A],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        (None, None, None),
        "a cleared override must leave no orphan reason/actor/timestamp behind"
    );
}

#[test]
fn an_override_on_an_absent_session_reports_false_rather_than_erroring() {
    let db = Db::open_memory().unwrap();
    assert!(
        !db.set_scope_override(UUID_A, OVERRIDE_WORK, "nobody home", "saidler@desk", now())
            .unwrap()
    );
}

// ---------------------------------------------------------------------------------------------
// Mutation-driven coverage (Phase 5): the write methods' return values, which callers use to tell
// "I changed something" from "there was nothing to change".
// ---------------------------------------------------------------------------------------------

/// KILLS: `replace > with >= in Db::record_enrich_skip`.
///
/// The bool is the observable difference between the guarded UPDATE and the bare one it replaced. A
/// mutant returning `true` unconditionally would make the no-change guard untestable from the
/// outside, which is how the original bare UPDATE went unnoticed in the first place.
#[test]
fn record_enrich_skip_reports_whether_it_actually_changed_anything() {
    use crate::export::EnrichStatus;

    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);

    assert!(
        db.record_enrich_skip(UUID_A, "personal", Some(3), EnrichStatus::SkippedPersonal)
            .unwrap(),
        "the first write changes the row"
    );
    assert!(
        !db.record_enrich_skip(UUID_A, "personal", Some(3), EnrichStatus::SkippedPersonal)
            .unwrap(),
        "an identical second write must report NO change, or the export cursor churns forever"
    );
    assert!(
        db.record_enrich_skip(UUID_A, "personal", None, EnrichStatus::SkippedPersonal)
            .unwrap(),
        "a different scope_version IS a change, and `IS NOT` is what makes NULL compare correctly"
    );
    assert!(
        !db.record_enrich_skip(UUID_B, "personal", None, EnrichStatus::SkippedPersonal)
            .unwrap(),
        "an absent session changes nothing"
    );
}

/// KILLS: `replace > with >= in Db::record_enrich_failure`.
///
/// Unlike the skip writer this one has no no-change guard (it bumps `attempts` every call, which is
/// the point), so its bool means "the session exists". A mutant returning `true` for an absent
/// session would report a charged attempt that never happened.
#[test]
fn record_enrich_failure_reports_whether_the_session_exists() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    assert!(db.record_enrich_failure(UUID_A, "work", "boom").unwrap());
    assert!(
        !db.record_enrich_failure(UUID_B, "work", "boom").unwrap(),
        "an absent session cannot be charged an attempt"
    );
}

// ---------------------------------------------------------------------------------------------
// The override re-offers its row: the F1 population, all five directions.
//
// Every test here seeds the state that IS the bug -- `enrich_status = 'skipped-personal'` AND
// `scope_version = SCOPE_VERSION` -- because that pair is what `Db::enrich_candidates` excludes.
// `a_scope_override_beats_a_refusal_in_both_directions` (`sessions/src/enrich/tests.rs`) sets an
// override on a FRESH row whose `scope_version` is already NULL, so it exercises the classifier's
// override branch and never the candidacy predicate that blocks in production.
// ---------------------------------------------------------------------------------------------

/// Enough of the enrich params that `enrich_candidates` is exercised the way a real pass calls it:
/// `all = false` is the whole point, since `--all` is the broken workaround being replaced.
const MAX_ATTEMPTS: i64 = 3;
const PROMPT_VERSION: i64 = 1;

/// Put a row into the exact state a normal enrich pass leaves a wrongly-personal session in.
fn seed_skipped_personal(db: &Db, session_id: &str) {
    seed(db, session_id);
    db.record_enrich_skip(
        session_id,
        OVERRIDE_PERSONAL,
        Some(session::SCOPE_VERSION),
        crate::EnrichStatus::SkippedPersonal,
    )
    .unwrap();
}

/// Enrich the row for real, so `enriched_at` and `prompt_version` are both set -- the shape that
/// must NOT be re-offered, or the fix becomes a re-enrich storm.
fn seed_enriched(db: &Db, session_id: &str) {
    seed(db, session_id);
    db.set_enrichment(
        session_id,
        &crate::EnrichSuccess {
            summary: "already sent",
            tags: None,
            scope: OVERRIDE_WORK,
            enriched_modified: dt("2026-06-21T10:00:00Z"),
            enrich_model: "test-model",
            prompt_version: PROMPT_VERSION,
            redaction_count: 0,
            tokens_in: 0,
            tokens_out: 0,
        },
        now(),
    )
    .unwrap();
}

fn is_candidate(db: &Db, session_id: &str) -> bool {
    db.enrich_candidates(None, PROMPT_VERSION, MAX_ATTEMPTS, false)
        .unwrap()
        .iter()
        .any(|r| r.session_id == session_id)
}

fn scope_version_of(db: &Db, session_id: &str) -> Option<i64> {
    db.conn
        .query_row(
            "SELECT scope_version FROM sessions WHERE session_id = ?1",
            params![session_id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .unwrap()
}

/// F1 itself. Before the fix the override wrote four columns, the row stayed excluded, and the
/// operator's only recourse was `--all` (which sets `force`, re-enriching the whole catalog and
/// clobbering every manual tag).
///
/// BITES: drop `scope_version = NULL` from `set_scope_override` and this fails.
#[test]
fn setting_work_on_a_skipped_personal_row_re_offers_it_without_all() {
    let db = Db::open_memory().unwrap();
    seed_skipped_personal(&db, UUID_A);
    assert!(
        !is_candidate(&db, UUID_A),
        "precondition: a skipped-personal row at the current scope_version is excluded"
    );

    assert!(
        db.set_scope_override(UUID_A, OVERRIDE_WORK, "F1 repro", "tester@desk", now())
            .unwrap()
    );

    assert_eq!(scope_version_of(&db, UUID_A), None);
    assert!(
        is_candidate(&db, UUID_A),
        "an operator override must re-offer the row it exists to rescue"
    );
}

/// The mirror direction the shakedown missed. Force personal -> a normal pass records
/// `skipped-personal` + `scope_version` -> `--clear` restores rule-based classification, which may
/// now say work, and without the fix the row is excluded from ever being asked.
///
/// BITES: drop the `CASE` clause from `clear_scope_override` and this fails.
#[test]
fn clearing_an_existing_override_re_offers_the_row() {
    let db = Db::open_memory().unwrap();
    seed_skipped_personal(&db, UUID_A);
    db.set_scope_override(UUID_A, OVERRIDE_PERSONAL, "forced personal", "tester@desk", now())
        .unwrap();
    // Re-record the skip, as a normal pass would, so the row is back in the blocked state.
    db.record_enrich_skip(
        UUID_A,
        OVERRIDE_PERSONAL,
        Some(session::SCOPE_VERSION),
        crate::EnrichStatus::SkippedPersonal,
    )
    .unwrap();
    assert!(!is_candidate(&db, UUID_A), "precondition: blocked again");

    assert!(db.clear_scope_override(UUID_A).unwrap());

    assert_eq!(scope_version_of(&db, UUID_A), None);
    assert!(
        is_candidate(&db, UUID_A),
        "clearing an override must re-offer the row, same as setting one"
    );
}

/// The hole the review panel found. `clear_scope_override` updates ANY existing session, override
/// or not, so an UNCONDITIONAL `scope_version = NULL` would turn `scope --clear` on a row with no
/// override into a hidden "re-offer this row" command -- reachable against every `skipped-personal`
/// row in the catalog (1018 of them on the live catalog, against 0 overrides) via a nominal no-op.
///
/// BITES: make `clear_scope_override`'s write unconditional and this fails.
#[test]
fn clearing_with_no_override_present_leaves_scope_version_untouched() {
    let db = Db::open_memory().unwrap();
    seed_skipped_personal(&db, UUID_A);
    assert_eq!(scope_version_of(&db, UUID_A), Some(session::SCOPE_VERSION));

    // Returns true because the SESSION exists -- the documented meaning of the bool, deliberately
    // preserved rather than flipped to "an override existed".
    assert!(db.clear_scope_override(UUID_A).unwrap());

    assert_eq!(
        scope_version_of(&db, UUID_A),
        Some(session::SCOPE_VERSION),
        "a no-op clear must not silently re-offer the row"
    );
    assert!(!is_candidate(&db, UUID_A), "a no-op clear must not re-offer the row");
}

/// The direction that already worked, asserted so the fix cannot regress it: forcing `personal` on
/// a plain candidate leaves it a candidate, and the routing gate then skips it.
#[test]
fn setting_personal_on_a_plain_candidate_keeps_it_a_candidate() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    assert!(is_candidate(&db, UUID_A), "precondition: a fresh row is a candidate");

    db.set_scope_override(UUID_A, OVERRIDE_PERSONAL, "forced personal", "tester@desk", now())
        .unwrap();

    assert!(is_candidate(&db, UUID_A));
}

/// The re-enrich-storm guard. `scope_version` is NOT one of the second candidacy clause's
/// disjuncts (`enriched_at IS NULL OR modified > enriched_modified OR prompt_version < ?`), so
/// NULLing it cannot resurrect a row whose transcript has already been sent. Both directions.
#[test]
fn an_already_enriched_row_is_not_re_offered_by_either_direction() {
    let db = Db::open_memory().unwrap();
    seed_enriched(&db, UUID_A);
    seed_enriched(&db, UUID_B);
    assert!(!is_candidate(&db, UUID_A), "precondition: an enriched row is excluded");

    db.set_scope_override(UUID_A, OVERRIDE_PERSONAL, "wrong scope", "tester@desk", now())
        .unwrap();
    assert!(!is_candidate(&db, UUID_A), "--set must not re-offer an enriched row");

    db.set_scope_override(UUID_B, OVERRIDE_WORK, "wrong scope", "tester@desk", now())
        .unwrap();
    db.clear_scope_override(UUID_B).unwrap();
    assert!(!is_candidate(&db, UUID_B), "--clear must not re-offer an enriched row");
}

/// `--set personal` on an already-enriched row warns, because the transcript has already been sent
/// and an override cannot un-send it. The CLI reads presence through this accessor.
#[test]
fn enriched_at_of_reports_presence_for_the_already_sent_warning() {
    let db = Db::open_memory().unwrap();
    seed(&db, UUID_A);
    assert_eq!(db.enriched_at_of(UUID_A).unwrap(), None);

    seed_enriched(&db, UUID_B);
    assert_eq!(
        db.enriched_at_of(UUID_B).unwrap().as_deref(),
        Some("2026-07-31T12:00:00+00:00")
    );

    // An absent session is `None`, never an error.
    assert_eq!(db.enriched_at_of("00000000-0000-0000-0000-000000000000").unwrap(), None);
}

// ---------------------------------------------------------------------------------------------
// routing_summary: decisions, not conditions.
//
// `routing_summary` had ZERO tests before this. Each one below asserts a DECISION -- what the
// classifier actually concluded -- because the defect was that a SQL condition count was being read
// as a decision count: `probe-refused` read 326 on the live catalog while the number of decisions a
// probe refusal made was 0.
// ---------------------------------------------------------------------------------------------

use common::repo::host::{HostPolicy, HostResolver};
use std::collections::HashMap;

/// A resolver with a fixed alias table, the pattern from `common/src/repo/host/tests.rs:28`.
struct FakeResolver(HashMap<String, String>);

impl FakeResolver {
    fn new(pairs: &[(&str, &str)]) -> Self {
        Self(
            pairs
                .iter()
                .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
                .collect(),
        )
    }
}

impl HostResolver for FakeResolver {
    fn hostname(&self, host: &str) -> Option<String> {
        self.0.get(host).cloned()
    }
}

/// A resolver that answers nothing, standing in for the NULL-resolver draft the panel killed.
struct NullResolver;

impl HostResolver for NullResolver {
    fn hostname(&self, _: &str) -> Option<String> {
        None
    }
}

const WORK_SLUG: &str = "tatari-tv/philo";
/// A cwd with a `repos/<work-org>` anchor: the cwd rule decides these outright, BEFORE the classifier
/// ever reaches the branch that reads `repo_probe` or `repo_host`. That gap is the whole defect.
const WORK_ANCHORED_CWD: &str = "/home/saidler/repos/tatari-tv/philo";
/// A cwd with NO `repos` component at all, so it is "unclassifiable" and the evidence gets a say.
/// This is the only shape that can REACH a refusal.
const UNANCHORED_CWD: &str = "/home/saidler/scratch/philo";
const PROBE_STAMP: &str = "not-a-repo@2026-07-31T12:00:00+00:00";

/// The routing columns a classification reads, so a test states the row's SHAPE rather than issuing
/// five UPDATEs inline.
#[derive(Default)]
struct Shape<'a> {
    cwd: Option<&'a str>,
    repo: Option<&'a str>,
    repo_source: Option<&'a str>,
    repo_probe: Option<&'a str>,
    repo_host: Option<&'a str>,
    scope_override: Option<&'a str>,
}

fn seed_shape(db: &Db, session_id: &str, s: &Shape<'_>) {
    seed(db, session_id);
    db.conn
        .execute(
            "UPDATE sessions SET cwd = ?2, repo = ?3, repo_source = ?4, repo_probe = ?5, \
             repo_host = ?6, scope_override = ?7 WHERE session_id = ?1",
            params![
                session_id,
                s.cwd,
                s.repo,
                s.repo_source,
                s.repo_probe,
                s.repo_host,
                s.scope_override
            ],
        )
        .unwrap();
}

fn github_only() -> Vec<String> {
    vec!["github.com".to_string()]
}

/// The single row's basis. Asserts the catalog holds exactly one row, so a stray fixture cannot make
/// a wrong answer look right.
fn sole_basis<R: HostResolver>(db: &Db, hosts: &mut HostPolicy<R>) -> Basis {
    let summary = db.routing_summary_with(hosts).unwrap();
    assert_eq!(summary.decisions_total(), 1, "expected exactly one row in the catalog");
    let found: Vec<Basis> = BASIS_ORDER
        .iter()
        .copied()
        .filter(|b| summary.by_basis[basis_index(*b)] == 1)
        .collect();
    assert_eq!(found.len(), 1, "exactly one basis must be non-zero");
    found[0]
}

/// `BASIS_ORDER` and `basis_index` must agree, or a count prints under the wrong label.
///
/// BITES: drop a variant from `BASIS_ORDER`, or swap two `basis_index` arms.
#[test]
fn basis_order_and_index_are_a_bijection_onto_the_array() {
    assert_eq!(BASIS_ORDER.len(), BASIS_COUNT);
    let mut seen = [false; BASIS_COUNT];
    for b in BASIS_ORDER {
        let i = basis_index(b);
        assert!(!seen[i], "two variants map to index {i}");
        seen[i] = true;
    }
    assert!(seen.iter().all(|s| *s), "some array slot has no variant");
    // Labels and remedies are non-empty for every variant, so no count can print bare.
    for b in BASIS_ORDER {
        assert!(!basis_label(b).is_empty());
        assert!(!basis_remedy(b).is_empty());
    }
}

/// Test 1. The decisions group SUMS to the catalog row count, which is the invariant that makes the
/// whole thing self-checking: `classify_with_evidence` returns exactly one `Basis` on every path.
///
/// BITES: give any classifier path no basis, or double-count one row.
#[test]
fn the_basis_tally_sums_to_the_catalog_row_count() {
    let db = Db::open_memory().unwrap();
    // Six shapes, one per basis, so the sum is not trivially satisfied by six identical rows.
    let shapes: [(&str, Shape<'_>); 6] = [
        (
            "10000000-0000-4000-8000-000000000001",
            Shape {
                scope_override: Some(OVERRIDE_WORK),
                ..Shape::default()
            },
        ),
        (
            "10000000-0000-4000-8000-000000000002",
            Shape {
                cwd: Some(WORK_ANCHORED_CWD),
                ..Shape::default()
            },
        ),
        (
            "10000000-0000-4000-8000-000000000003",
            Shape {
                cwd: Some(UNANCHORED_CWD),
                repo: Some(WORK_SLUG),
                repo_source: Some("git-origin"),
                repo_host: Some("github.com"),
                ..Shape::default()
            },
        ),
        (
            "10000000-0000-4000-8000-000000000004",
            Shape {
                cwd: Some(UNANCHORED_CWD),
                ..Shape::default()
            },
        ),
        (
            "10000000-0000-4000-8000-000000000005",
            Shape {
                cwd: Some(UNANCHORED_CWD),
                repo: Some(WORK_SLUG),
                repo_source: Some("git-origin"),
                repo_host: Some("evil.example.com"),
                ..Shape::default()
            },
        ),
        (
            "10000000-0000-4000-8000-000000000006",
            Shape {
                cwd: Some(UNANCHORED_CWD),
                repo: Some(WORK_SLUG),
                repo_source: Some("git-origin"),
                repo_host: Some("github.com"),
                repo_probe: Some(PROBE_STAMP),
                ..Shape::default()
            },
        ),
    ];
    for (id, shape) in &shapes {
        seed_shape(&db, id, shape);
    }

    let mut hosts = HostPolicy::with_resolver(&github_only(), NullResolver);
    let summary = db.routing_summary_with(&mut hosts).unwrap();

    let rows: usize = db
        .conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0))
        .unwrap() as usize;
    assert_eq!(summary.decisions_total(), rows);
    assert_eq!(rows, 6);
    // Every basis was actually exercised, so the sum is a real distribution and not one bucket.
    for b in BASIS_ORDER {
        assert_eq!(
            summary.by_basis[basis_index(b)],
            1,
            "{} should have exactly one row",
            basis_label(b)
        );
    }
}

/// Test 2. THE DEFECT, asserted directly. A recorded `repo_probe` satisfies the old
/// `repo_probe IS NOT NULL` count, but the cwd anchor decided this row long before the classifier
/// reached the probe branch. 326 rows on the live catalog are this shape.
///
/// BITES: count the row as `ProbeRefused` (which is what the old SQL did) and this fails.
#[test]
fn a_probe_stamp_under_a_work_anchored_cwd_counts_as_cwd_anchor() {
    let db = Db::open_memory().unwrap();
    seed_shape(
        &db,
        UUID_A,
        &Shape {
            cwd: Some(WORK_ANCHORED_CWD),
            repo: Some(WORK_SLUG),
            repo_source: Some("git-origin"),
            repo_probe: Some(PROBE_STAMP),
            repo_host: Some("github.com"),
            ..Shape::default()
        },
    );

    let mut hosts = HostPolicy::with_resolver(&github_only(), NullResolver);
    assert_eq!(sole_basis(&db, &mut hosts), Basis::CwdAnchor);

    // The CONDITION is still reported, under its own honest name, because `--clear-probe` is the
    // remedy for a stale stamp and an operator has no other way to find these rows.
    assert_eq!(db.routing_summary_with(&mut hosts).unwrap().probe_recorded, 1);
}

/// Test 3. The shape that GENUINELY reaches the probe refusal, plus the host flip on the same row.
#[test]
fn a_row_that_reaches_the_refusal_counts_as_probe_refused_and_the_host_flips_it() {
    let db = Db::open_memory().unwrap();
    seed_shape(
        &db,
        UUID_A,
        &Shape {
            cwd: Some(UNANCHORED_CWD),
            repo: Some(WORK_SLUG),
            repo_source: Some("git-origin"),
            repo_probe: Some(PROBE_STAMP),
            repo_host: Some("github.com"),
            ..Shape::default()
        },
    );
    let mut hosts = HostPolicy::with_resolver(&github_only(), NullResolver);
    assert_eq!(sole_basis(&db, &mut hosts), Basis::ProbeRefused);

    // Same row, probe cleared: the allowlisted host lets the remote decide.
    db.conn.execute("UPDATE sessions SET repo_probe = NULL", []).unwrap();
    assert_eq!(sole_basis(&db, &mut hosts), Basis::GitOrigin);

    // Same row again, host no longer allowlisted: the host refuses before anything else.
    db.conn
        .execute("UPDATE sessions SET repo_host = 'evil.example.com'", [])
        .unwrap();
    assert_eq!(sole_basis(&db, &mut hosts), Basis::HostRefused);
}

/// Test 4. Both refusal conditions on one row. HOST wins, pinning the classifier's precedence
/// (`session/src/scope.rs:288` runs before `:296`).
///
/// This is also what AC3's SQL had to learn: without a `repo_host` clause the criterion
/// double-counts the moment this shape appears.
///
/// BITES: swap the host and probe checks in `classify_with_evidence` and this fails.
#[test]
fn a_row_carrying_both_refusals_counts_as_host_refused() {
    let db = Db::open_memory().unwrap();
    seed_shape(
        &db,
        UUID_A,
        &Shape {
            cwd: Some(UNANCHORED_CWD),
            repo: Some(WORK_SLUG),
            repo_source: Some("git-origin"),
            repo_probe: Some(PROBE_STAMP),
            repo_host: Some("evil.example.com"),
            ..Shape::default()
        },
    );
    let mut hosts = HostPolicy::with_resolver(&github_only(), NullResolver);
    assert_eq!(sole_basis(&db, &mut hosts), Basis::HostRefused);
}

/// Test 5. The test that would have caught the null-resolver draft.
///
/// An SSH alias resolving to an allowlisted host confers work AT THE GATE. If `doctor` compared
/// literally instead, this row would read `GitOrigin` at the gate and `HostRefused` here -- the exact
/// defect P2 exists to remove, one layer down.
///
/// BITES: swap `FakeResolver` for `NullResolver` (which is the null-resolver draft) and this fails.
#[test]
fn an_ssh_alias_resolving_to_an_allowlisted_host_counts_as_git_origin() {
    let db = Db::open_memory().unwrap();
    seed_shape(
        &db,
        UUID_A,
        &Shape {
            cwd: Some(UNANCHORED_CWD),
            repo: Some(WORK_SLUG),
            repo_source: Some("git-origin"),
            repo_host: Some("github-work"),
            ..Shape::default()
        },
    );

    let mut real = HostPolicy::with_resolver(&github_only(), FakeResolver::new(&[("github-work", "github.com")]));
    assert_eq!(sole_basis(&db, &mut real), Basis::GitOrigin);

    // The draft the panel killed, for contrast: a literal-only comparison refuses the same row.
    let mut null = HostPolicy::with_resolver(&github_only(), NullResolver);
    assert_eq!(sole_basis(&db, &mut null), Basis::HostRefused);
}

/// The free cross-check available on day one: `Override` is step 0 and beats every rule, so that ONE
/// condition WAS decision-accurate. If the tally and the old SQL count disagree, the tally is wrong.
#[test]
fn the_override_basis_count_equals_the_override_sql_count() {
    let db = Db::open_memory().unwrap();
    seed_shape(
        &db,
        UUID_A,
        &Shape {
            cwd: Some(WORK_ANCHORED_CWD),
            scope_override: Some(OVERRIDE_PERSONAL),
            ..Shape::default()
        },
    );
    seed_shape(
        &db,
        UUID_B,
        &Shape {
            cwd: Some(WORK_ANCHORED_CWD),
            ..Shape::default()
        },
    );

    let mut hosts = HostPolicy::with_resolver(&github_only(), NullResolver);
    let summary = db.routing_summary_with(&mut hosts).unwrap();
    let sql: usize = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE scope_override IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap() as usize;
    assert_eq!(summary.by_basis[basis_index(Basis::Override)], sql);
    assert_eq!(sql, 1);
}

/// A malformed `outcome_json` must WARN and count the row as evidence-absent, never abort the scan.
/// `doctor` is the command an operator runs when something is already broken.
///
/// BITES: propagate the parse error out of `evidence_from_row` and this errors instead of counting.
#[test]
fn a_malformed_outcome_json_does_not_abort_the_scan() {
    let db = Db::open_memory().unwrap();
    seed_shape(
        &db,
        UUID_A,
        &Shape {
            cwd: Some(UNANCHORED_CWD),
            ..Shape::default()
        },
    );
    db.conn
        .execute("UPDATE sessions SET outcome_json = '{not json'", [])
        .unwrap();

    let mut hosts = HostPolicy::with_resolver(&github_only(), NullResolver);
    // Evidence-absent -> the touch-set tail decides, personal and provisional.
    assert_eq!(sole_basis(&db, &mut hosts), Basis::TouchSet);
}

/// An unreadable `repo_source` must WARN and classify WITHOUT the remote signal, exactly as the
/// enrich path does -- both now go through `crate::routing::parse_repo_source`.
///
/// BITES: `.ok()` the parse (the pre-register-item-6 form) and the row still lands on TouchSet, so
/// this test pins the LOUD path via the classification rather than the log: a row whose only work
/// signal is its unreadable-source remote must NOT count as `GitOrigin`.
#[test]
fn an_unreadable_repo_source_classifies_without_the_remote() {
    let db = Db::open_memory().unwrap();
    seed_shape(
        &db,
        UUID_A,
        &Shape {
            cwd: Some(UNANCHORED_CWD),
            repo: Some(WORK_SLUG),
            ..Shape::default()
        },
    );
    db.set_raw_repo_source_for_test(UUID_A, "future-rule-5").unwrap();

    let mut hosts = HostPolicy::with_resolver(&github_only(), NullResolver);
    assert_eq!(sole_basis(&db, &mut hosts), Basis::TouchSet);
}
