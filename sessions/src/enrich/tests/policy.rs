//! Schema v14 through the real sweep: a `reposlugs-ptns` or `repo-roots` change re-offers the rows it
//! can reach, an unchanged policy writes nothing, and a re-pointed checkout loses the old slug's
//! authority. Design: `docs/design/2026-09-27-reposlugs-ptns-from-config.md` (Phase 3).
//!
//! On disk, not `open_memory`, so a second connection can read `export_meta.revision` (the cursor
//! every write advances) and stand in for an older binary's writes.

use std::process::Command;

use common::repo::ptn::ReposlugPtn;
use session::ScopePolicy;

use super::*;

const ROOT: &str = "/home/saidler/repos";
/// `scottidler` has no include under `["tatari-tv/*"]`, so this is Personal, SETTLED, by the anchor.
const UNLISTED_CWD: &str = "/home/saidler/repos/scottidler/claude";
const WIDE: &[&str] = &["tatari-tv/*"];

fn policy(raw: &[&str]) -> ScopePolicy {
    let ptns: Vec<ReposlugPtn> = raw.iter().map(|r| ReposlugPtn::parse(r).unwrap()).collect();
    ScopePolicy::new(&[PathBuf::from(ROOT)], &ptns)
}

fn opts(raw: &[&str]) -> EnrichOptions {
    EnrichOptions {
        scope_policy: policy(raw),
        ..Default::default()
    }
}

struct Catalog {
    db: Db,
    path: PathBuf,
    tmp: tempfile::TempDir,
}

impl Catalog {
    fn new() -> Self {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sessions.db");
        Self {
            db: Db::open_at(&path).unwrap(),
            path,
            tmp,
        }
    }

    /// A row with a real transcript, so a Work decision reaches the completer. `minutes_ago` orders
    /// the sweep, which runs newest first.
    fn add(&self, id: &str, cwd: &str, minutes_ago: i64) {
        let parent = write_transcript(self.tmp.path(), id, "some content worth summarizing");
        let mut rec = parsed_record(self.tmp.path(), id, cwd, &parent);
        rec.modified = dt("2026-06-21T10:00:00Z") - chrono::Duration::minutes(minutes_ago);
        self.db.upsert_session(&rec, "desk").unwrap();
    }

    /// Run `stmt` on a second connection: what an older binary, or a crash, leaves behind.
    fn sql(&self, stmt: &str) {
        rusqlite::Connection::open(&self.path)
            .unwrap()
            .execute(stmt, [])
            .unwrap();
    }

    fn revision(&self) -> i64 {
        rusqlite::Connection::open(&self.path)
            .unwrap()
            .query_row("SELECT revision FROM export_meta WHERE id = 0", [], |r| r.get(0))
            .unwrap()
    }

    fn stored(&self, id: &str) -> (Option<i64>, Option<String>) {
        rusqlite::Connection::open(&self.path)
            .unwrap()
            .query_row(
                "SELECT scope_version, scope_policy FROM sessions WHERE session_id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
    }

    fn sweep(&self, fake: &Fake, opts: &EnrichOptions) -> crate::model::EnrichStats {
        enrich(&self.db, Some(fake), opts).unwrap()
    }
}

/// The headline: a settled-personal row is re-offered by a widening, classifies Work, and is sent,
/// with no code change and no `SCOPE_VERSION` bump. Around it, an unchanged policy writes NOTHING.
///
/// BITES: drop `OR s.scope_policy IS NOT ?4` from `enrich_candidates` and the widened pass
/// considers 0 rows.
#[test]
fn a_widening_re_offers_a_settled_personal_row_and_sends_it() {
    let c = Catalog::new();
    c.add(UUID_A, UNLISTED_CWD, 0);
    set_git_origin(&c.db, UUID_A, "scottidler/claude");

    let fake = Fake::ok(&["x"]);
    let first = c.sweep(&fake, &opts(WIDE));
    assert_eq!((first.skipped_personal, fake.calls()), (1, 0));
    assert_eq!(
        c.stored(UUID_A),
        (Some(session::SCOPE_VERSION), Some(policy(WIDE).fingerprint()))
    );

    let rev = c.revision();
    assert_eq!(
        c.sweep(&fake, &opts(WIDE)).considered,
        0,
        "settled under an unchanged policy"
    );
    assert_eq!(c.revision(), rev, "a second pass under the same policy writes 0 rows");

    let widened = &["tatari-tv/*", "scottidler/claude"];
    let stats = c.sweep(&fake, &opts(widened));
    assert_eq!((stats.considered, stats.enriched, fake.calls()), (1, 1, 1));
    assert_eq!(c.sweep(&fake, &opts(widened)).considered, 0);
}

/// Adding ONLY an exclude is a policy change: the row is re-offered once, re-settles personal under
/// the new fingerprint, and drops out.
#[test]
fn adding_only_an_exclude_re_offers_then_re_settles() {
    let c = Catalog::new();
    c.add(UUID_A, UNLISTED_CWD, 0);
    let fake = Fake::ok(&["x"]);
    c.sweep(&fake, &opts(WIDE));

    let narrowed = &["tatari-tv/*", "!tatari-tv/secret"];
    let rev = c.revision();
    let stats = c.sweep(&fake, &opts(narrowed));
    assert_eq!((stats.considered, stats.skipped_personal, fake.calls()), (1, 1, 0));
    assert_eq!(c.revision(), rev + 1, "one write: the new fingerprint");
    assert_eq!(c.sweep(&fake, &opts(narrowed)).considered, 0);
}

/// An `ok` row is not re-offered by a policy change: freshness, not policy, governs what was sent.
#[test]
fn an_ok_row_is_not_re_offered_by_a_policy_change() {
    let c = Catalog::new();
    c.add(UUID_A, WORK_CWD, 0);
    let fake = Fake::ok(&["x"]);
    assert_eq!(c.sweep(&fake, &opts(WIDE)).enriched, 1);
    assert_eq!(c.sweep(&fake, &opts(&["tatari-tv/*", "otto-rs/otto"])).considered, 0);
    assert_eq!(fake.calls(), 1);
}

/// A row a v13 binary settled carries `scope_version` and a NULL `scope_policy`: re-offered exactly
/// once, then settled.
#[test]
fn a_settled_v13_row_is_re_offered_exactly_once_then_settles() {
    let c = Catalog::new();
    c.add(UUID_A, UNLISTED_CWD, 0);
    let fake = Fake::ok(&["x"]);
    c.sweep(&fake, &opts(WIDE));
    c.sql("UPDATE sessions SET scope_policy = NULL");

    assert_eq!(
        c.sweep(&fake, &opts(WIDE)).considered,
        1,
        "NULL is not the current policy"
    );
    assert_eq!(c.sweep(&fake, &opts(WIDE)).considered, 0, "and now it is recorded");
}

/// A sweep aborted part-way (a dead transport) leaves every row it did not reach eligible, and the
/// next pass finishes them.
#[test]
fn an_interrupted_sweep_leaves_unvisited_rows_eligible() {
    const P1: &str = "aaaaaaaa-0000-4000-8000-000000000001";
    const W: &str = "aaaaaaaa-0000-4000-8000-000000000002";
    const P2: &str = "aaaaaaaa-0000-4000-8000-000000000003";
    const P3: &str = "aaaaaaaa-0000-4000-8000-000000000004";
    let c = Catalog::new();
    c.add(P1, "/home/saidler/repos/scottidler/p1", 0);
    c.add(P2, "/home/saidler/repos/scottidler/p2", 20);
    c.add(P3, "/home/saidler/repos/scottidler/p3", 30);
    c.sweep(&Fake::ok(&["x"]), &opts(WIDE));

    // A new work row, between P1 and P2 in sweep order. The dead transport aborts on it.
    c.add(W, "/home/saidler/repos/tatari-tv/w", 10);
    let widened = opts(&["tatari-tv/*", "otto-rs/otto"]);
    let dead = Flaky::new(&[], true, true);
    assert!(enrich(&c.db, Some(&dead), &widened).is_err());

    let fp = widened.scope_policy.fingerprint();
    let mut left: Vec<String> =
        c.db.enrich_candidates(None, ENRICH_PROMPT_VERSION, DEFAULT_MAX_ATTEMPTS, false, &fp)
            .unwrap()
            .into_iter()
            .map(|r| r.session_id)
            .collect();
    left.sort();
    assert_eq!(
        left,
        vec![W, P2, P3],
        "P1 was visited and re-settled; the rest were not"
    );

    let fake = Fake::ok(&["x"]);
    let stats = c.sweep(&fake, &widened);
    assert_eq!((stats.considered, stats.enriched, stats.skipped_personal), (3, 1, 2));
    assert_eq!(c.sweep(&fake, &widened).considered, 0);
}

/// `ok` -> narrowed `skipped-personal` (keeping `enriched_at`) -> widened: re-offered and re-sent.
///
/// BITES: restore the pre-v14 two-`AND` predicate and the widened pass considers 0 rows, because
/// `enriched_at` is set and the row is otherwise fresh.
#[test]
fn ok_then_narrowed_then_widened_re_offers_the_row() {
    let c = Catalog::new();
    c.add(UUID_A, WORK_CWD, 0);
    let fake = Fake::ok(&["x"]);
    assert_eq!(c.sweep(&fake, &opts(WIDE)).enriched, 1);

    let narrow = &["otto-rs/*"];
    let all = EnrichOptions {
        all: true,
        ..opts(narrow)
    };
    assert_eq!(c.sweep(&fake, &all).skipped_personal, 1);
    assert_eq!(
        c.sweep(&fake, &opts(narrow)).considered,
        0,
        "settled under the narrow policy"
    );

    let stats = c.sweep(&fake, &opts(WIDE));
    assert_eq!((stats.considered, stats.enriched, fake.calls()), (1, 1, 2));
}

/// A previously-enriched row that is now PROVISIONAL personal (an exclude hit its wide owner) is
/// re-offered on every pass, and the second of those passes writes nothing.
#[test]
fn a_previously_enriched_provisional_row_is_re_offered_each_pass_without_writing() {
    let c = Catalog::new();
    c.add(UUID_A, "/home/saidler/repos/tatari-tv/secret", 0);
    let fake = Fake::ok(&["x"]);
    assert_eq!(c.sweep(&fake, &opts(WIDE)).enriched, 1);

    let excluded = &["tatari-tv/*", "!tatari-tv/secret"];
    let all = EnrichOptions {
        all: true,
        ..opts(excluded)
    };
    assert_eq!(c.sweep(&fake, &all).skipped_personal, 1);
    assert_eq!(c.stored(UUID_A).0, None, "an exclude is provisional");

    assert_eq!(c.sweep(&fake, &opts(excluded)).considered, 1);
    let rev = c.revision();
    assert_eq!(c.sweep(&fake, &opts(excluded)).considered, 1, "still provisional");
    assert_eq!(c.revision(), rev, "an unchanged provisional decision writes 0 rows");
    assert_eq!(fake.calls(), 1, "never re-sent");
}

/// Down then up, twice (5 -> 4 -> 5 -> 4 -> 5). A v13 binary re-settles with `scope_version = 4` and
/// never touches `scope_policy`, so the row carries the CURRENT fingerprint; it is the
/// `SCOPE_VERSION` term that re-offers it, each time, and then it settles.
#[test]
fn down_then_up_twice_re_offers_rows_the_old_writer_re_settled() {
    let c = Catalog::new();
    c.add(UUID_A, UNLISTED_CWD, 0);
    let fake = Fake::ok(&["x"]);
    c.sweep(&fake, &opts(WIDE));
    let settled = (Some(session::SCOPE_VERSION), Some(policy(WIDE).fingerprint()));
    assert_eq!(c.stored(UUID_A), settled);

    for round in 1..=2 {
        c.sql("UPDATE sessions SET scope_version = 4");
        assert_eq!(c.sweep(&fake, &opts(WIDE)).considered, 1, "round {round}: re-offered");
        assert_eq!(c.stored(UUID_A), settled, "round {round}: re-settled");
        assert_eq!(c.sweep(&fake, &opts(WIDE)).considered, 0, "round {round}: settled");
    }
}

/// The host/slug pairing at the classifier seam: a recorded slug that differs from `repo` refuses,
/// one that matches (in any case) confers, and a NULL one keeps today's answer.
#[test]
fn a_host_observed_with_another_slug_refuses_work() {
    let p = policy(&["scottidler/claude"]);
    let decide = |repo_host_slug: Option<&str>| {
        let evidence = crate::db::ScopeEvidence {
            repo_host: Some("github.com".to_string()),
            repo_host_slug: repo_host_slug.map(str::to_string),
            ..Default::default()
        };
        crate::routing::classify_row(
            UUID_A,
            Some("/home/saidler/repos/scottidler/claude-feature"),
            Some("scottidler/claude"),
            Some("git-origin"),
            &evidence,
            &p,
            &mut HostPolicy::new(&["github.com".to_string()]),
        )
        .decision
    };
    let refused = decide(Some("scottidler/second-brain"));
    assert_eq!(
        (refused.scope, refused.basis),
        (session::Scope::Personal, session::Basis::HostRefused)
    );
    for paired in [Some("scottidler/claude"), Some("ScottIdler/Claude"), None] {
        let d = decide(paired);
        assert_eq!(
            (d.scope, d.basis),
            (session::Scope::Work, session::Basis::GitOrigin),
            "{paired:?}"
        );
    }
}

fn git(dir: &Path, args: &[&str]) {
    let s = Command::new("git").args(args).current_dir(dir).status().unwrap();
    assert!(s.success(), "git {args:?}");
}

/// End to end: index a checkout with remote `scottidler/claude`, re-point its origin at
/// `scottidler/second-brain` on the same host, re-index. The rank-0 `repo` keeps the old slug, the
/// pairing column carries the new one, and the row is Personal `HostRefused`: enrich sends nothing
/// and export carries the refused scope. Re-pointing back makes it Work again.
///
/// BITES: drop the `repo_host_slug` refusal from `routing::host_confers_work` and the re-pointed row
/// is sent.
#[test]
fn a_re_pointed_checkout_loses_the_old_slugs_authority() {
    let c = Catalog::new();
    let base = c.tmp.path().canonicalize().unwrap();
    let roots = vec![base.join("repos")];
    let checkout = base.join("repos").join("scottidler").join("claude");
    std::fs::create_dir_all(&checkout).unwrap();
    git(&checkout, &["init", "-q"]);
    git(
        &checkout,
        &["remote", "add", "origin", "git@github.com:scottidler/claude.git"],
    );
    let projects = base.join("projects");
    let transcript = projects.join("proj").join(format!("{UUID_A}.jsonl"));
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    let line = serde_json::json!({
        "type": "user",
        "cwd": checkout,
        "timestamp": "2026-06-20T10:00:00Z",
        "sessionId": UUID_A,
        "message": { "content": "work on the claude repo" }
    });
    std::fs::write(&transcript, format!("{line}\n")).unwrap();
    let opts = EnrichOptions {
        scope_policy: ScopePolicy::new(&roots, &[ReposlugPtn::parse("scottidler/claude").unwrap()]),
        ..Default::default()
    };
    let slug = |c: &Catalog| c.db.scope_evidence(UUID_A).unwrap().repo_host_slug;

    crate::index::reindex(&c.db, &projects, &roots).unwrap();
    assert_eq!(slug(&c).as_deref(), Some("scottidler/claude"));

    git(
        &checkout,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:scottidler/second-brain.git",
        ],
    );
    crate::index::reindex(&c.db, &projects, &roots).unwrap();
    assert_eq!(slug(&c).as_deref(), Some("scottidler/second-brain"));
    let rec = c.db.get(UUID_A).unwrap().unwrap();
    assert_eq!(
        rec.repo.as_deref(),
        Some("scottidler/claude"),
        "rank 0 is never replaced"
    );

    let d = crate::routing::classify_row(
        UUID_A,
        rec.cwd.as_deref(),
        rec.repo.as_deref(),
        rec.repo_source.as_deref(),
        &c.db.scope_evidence(UUID_A).unwrap(),
        &opts.scope_policy,
        &mut HostPolicy::new(&opts.work_remote_hosts),
    )
    .decision;
    assert_eq!(
        (d.scope, d.basis),
        (session::Scope::Personal, session::Basis::HostRefused)
    );

    let ctx = ExportContext {
        now: dt("2026-07-01T00:00:00Z"),
        host: "desk".into(),
        dormant_after: chrono::Duration::days(7),
        scope_policy: opts.scope_policy.clone(),
        work_remote_hosts: opts.work_remote_hosts.clone(),
    };
    let exported = |c: &Catalog| {
        c.db.export(&ExportFilters::default(), &ctx).unwrap().sessions[0]
            .scope
            .clone()
    };
    assert_eq!(
        exported(&c),
        "personal",
        "the undecided row's fallback is the refused scope"
    );

    let fake = Fake::ok(&["x"]);
    let stats = c.sweep(&fake, &opts);
    assert_eq!((stats.skipped_personal, fake.calls()), (1, 0), "0 bodies sent");
    assert_eq!(exported(&c), "personal");

    git(
        &checkout,
        &["remote", "set-url", "origin", "git@github.com:scottidler/claude.git"],
    );
    crate::index::reindex(&c.db, &projects, &roots).unwrap();
    assert_eq!(slug(&c).as_deref(), Some("scottidler/claude"));
    let stats = c.sweep(&fake, &opts);
    assert_eq!((stats.enriched, fake.calls()), (1, 1), "paired again, the row is Work");
}
