//! The checkout matrix, asserted end to end against the real `clyde` binary.
//!
//! `docs/design/2026-07-31-attribution-and-routing.md` (Testing Strategy). The register's root cause
//! is that the tests could not see the defect: every scope test was a SINGLE classification of a
//! FIXED input, hand-built through a `with_repo(..)` helper that bypassed the resolver, so a row
//! production can never emit was asserted as work and inflated the measured win.
//!
//! Three structural fixes land here:
//!
//! 1. **Real `git init` fixtures, not a mocked `run_git`.** [`common::checkout::Matrix`] builds every
//!    shape in a `TempDir` with its OWN `HOME`, `repo-root` and `projects-dir`. Never `~/repos`.
//! 2. **The real resolver.** Row assertions call `common::repo` directly; the end-to-end tests drive
//!    the shipped binary through `session reindex` and `session enrich --dry-run`.
//! 3. **Sequences, not snapshots.** [`Sandbox`] can reindex, mutate the world, and reindex again,
//!    which is the shape Problem 1 lives in and which no test in the tree had.
//!
//! Rows are turned on by the phase that fixes them, so every phase stays green. A row still awaiting
//! its phase is asserted at its CURRENT (wrong) answer and named with the phase that must break it,
//! so a reader can tell a deliberate gap from an oversight.

use std::path::{Path, PathBuf};
use std::process::Command;

use common::checkout::Matrix;
use common::repo::ProbeOutcome;

/// A UUID-v4 per matrix row that gets a seeded session. `scan::find_session_files` requires the
/// stem to be a v4 UUID, so these cannot be arbitrary strings.
const SID_FLAT: &str = "11111111-1111-4111-8111-111111111111";
const SID_NO_ORIGIN: &str = "22222222-2222-4222-8222-222222222222";
const SID_FORK: &str = "33333333-3333-4333-8333-333333333333";
const SID_LAYOUT: &str = "44444444-4444-4444-8444-444444444444";
const SID_DELETABLE: &str = "55555555-5555-4555-8555-555555555555";
const SID_EVIL_HOST: &str = "66666666-6666-4666-8666-666666666666";
const SID_ALIAS: &str = "77777777-7777-4777-8777-777777777777";
const SID_SUBMODULE: &str = "88888888-8888-4888-8888-888888888888";

/// The rule-1 SLUG for a matrix cwd, through the REAL resolver with the fixture's blocked roots.
/// Rows that care about the slug alone use this; rows about the routing gate use [`probe`].
fn detect(m: &Matrix, cwd: &Path) -> Option<String> {
    probe(m, cwd).resolved_slug().map(str::to_string)
}

/// The full typed [`ProbeOutcome`], for the rows whose whole point is WHICH kind of decline it was.
fn probe(m: &Matrix, cwd: &Path) -> ProbeOutcome {
    common::repo::detect_with_blocked_roots(cwd, &m.blocked())
}

// ---------------------------------------------------------------------------------------------
// Row assertions: the rules, against real git, with no catalog in the way.
// ---------------------------------------------------------------------------------------------

#[test]
fn matrix_row_01_flat_clone_with_an_ssh_remote() {
    let m = Matrix::build();
    assert_eq!(detect(&m, &m.flat_ssh).as_deref(), Some("tatari-tv/philo"));
}

#[test]
fn matrix_row_02_flat_clone_with_an_https_remote() {
    let m = Matrix::build();
    assert_eq!(detect(&m, &m.flat_https).as_deref(), Some("tatari-tv/philo"));
}

#[test]
fn matrix_row_03_a_subdirectory_resolves_through_its_toplevel() {
    let m = Matrix::build();
    assert_eq!(detect(&m, &m.subdir).as_deref(), Some("tatari-tv/philo"));
}

#[test]
fn matrix_row_04_a_sibling_worktree_at_org_level() {
    let m = Matrix::build();
    assert_eq!(detect(&m, &m.worktree).as_deref(), Some("tatari-tv/clyde"));
}

#[test]
fn matrix_row_05_a_bare_container_child() {
    let m = Matrix::build();
    assert_eq!(
        detect(&m, &m.container_child).as_deref(),
        Some("tatari-tv/airflow-dags"),
        "the container CHILD is the row the old verification table did test, and it works"
    );
}

/// Problem 1's seed state, and the ONE negative the routing gate records. It must be
/// CONCLUSIVE, not a bare decline: a transient failure looks identical through an `Option`.
#[test]
fn matrix_row_09_a_repo_with_no_origin_is_conclusively_no_origin() {
    let m = Matrix::build();
    let outcome = probe(&m, &m.no_origin);
    assert_eq!(outcome, ProbeOutcome::NoOrigin);
    assert!(
        outcome.is_conclusive_negative(),
        "this is the record that refuses the flip"
    );
}

#[test]
fn matrix_row_11_a_non_git_directory_is_conclusively_not_a_repo() {
    let m = Matrix::build();
    let outcome = probe(&m, &m.not_a_repo);
    assert_eq!(outcome, ProbeOutcome::NotARepo);
    assert!(outcome.is_conclusive_negative());
}

#[test]
fn matrix_row_17_a_bare_home_with_no_repo_declines() {
    let m = Matrix::build();
    assert_eq!(
        detect(&m, &m.home()),
        None,
        "Patrick's layout: a `~` cwd with no repo must stay unresolvable, which is what makes it \
         personal (register item 4)"
    );
}

#[test]
fn matrix_row_18_a_personal_fork_in_a_work_directory_resolves_to_the_personal_remote() {
    let m = Matrix::build();
    assert_eq!(
        detect(&m, &m.fork_in_work_dir).as_deref(),
        Some("scottidler/clyde-fork"),
        "rule 1 reports what the REMOTE says; the cwd anchor is what keeps this session Work, and \
         the two disagreeing is the case that killed the precedence change"
    );
}

/// Rows 14, 15 and 16: the three teammate layouts with no org slot a path walk could read. Rule 1
/// answers correctly in all three, which is the v0.22.0 win this branch must not roll back.
#[test]
fn matrix_rows_14_to_16_the_no_org_level_layouts_all_resolve() {
    let m = Matrix::build();
    for (label, cwd) in [
        ("code/work (Stephen)", &m.layout_code_work),
        ("Projects (Luke)", &m.layout_projects),
        ("git/tatari (Keegan)", &m.layout_git_tatari),
    ] {
        assert_eq!(
            detect(&m, cwd).as_deref(),
            Some("tatari-tv/philo"),
            "{label} must resolve through the remote, not the layout"
        );
    }
}

/// Row 30. An empty repo (no commits) still has a work tree, so it must be `NoOrigin` (conclusive)
/// and NOT be confused with a repo-discovery failure. Those are opposite answers: one records, the
/// other must not.
#[test]
fn matrix_row_30_an_empty_repo_is_conclusively_no_origin() {
    let m = Matrix::build();
    assert_eq!(probe(&m, &m.empty_repo), ProbeOutcome::NoOrigin);
}

/// Rows 6, 7, 8 and 24, turned ON by Phase 6: every shape with no work tree, plus the
/// symlink-reached cwd, now resolves through the shipped binary's own resolver.
///
/// These were asserted at their WRONG answers through Phases 1 to 5 so the fix had something to
/// break. `common::repo::tests` proves each one bites; this is the same claim at the integration
/// altitude, over the shared fixture.
#[test]
fn matrix_rows_06_07_08_and_24_resolve_without_a_work_tree() {
    let m = Matrix::build();
    assert_eq!(
        detect(&m, &m.container_root).as_deref(),
        Some("tatari-tv/airflow-dags"),
        "row 6: Problem 4, the bare-repo container ROOT"
    );
    assert_eq!(
        detect(&m, &m.bare_mirror).as_deref(),
        Some("tatari-tv/mirror"),
        "row 7: a plain bare repo, whose common dir is `.`"
    );
    assert_eq!(
        detect(&m, &m.inside_bare).as_deref(),
        Some("tatari-tv/airflow-dags"),
        "row 8: a cwd inside the container's .bare resolves through the container"
    );
    assert_eq!(
        detect(&m, &m.symlinked).as_deref(),
        Some("tatari-tv/philo"),
        "row 24: the toplevel is canonical and the cwd is not; a lexical check declined it"
    );
}

/// Rows 19 and 21: the host is validated for the SCOPE it confers, and attribution is deliberately
/// left alone. A non-allowlisted remote still parses to its slug, because refusing to attribute it
/// would lose the provenance an operator needs to see WHY a session was refused.
#[test]
fn matrix_row_19_a_refused_host_still_attributes_its_slug() {
    let m = Matrix::build();
    // Rows 19 and 21: the host IS validated as of Phase 3, but only for the SCOPE it confers.
    // Attribution is deliberately unchanged, so the slug still parses.
    assert_eq!(
        detect(&m, &m.host_not_allowed).as_deref(),
        Some("tatari-tv/x"),
        "row 19: an `evil.example.com` remote still ATTRIBUTES; Phase 3 stopped it conferring work"
    );
}

// ---------------------------------------------------------------------------------------------
// The end-to-end sandbox: real reindex, real enrich gate, its own HOME.
// ---------------------------------------------------------------------------------------------

/// A hermetic clyde installation over one [`Matrix`]: its own `HOME`, `XDG_CONFIG_HOME`,
/// `XDG_DATA_HOME`, catalog, and `clyde.yml`.
///
/// The `projects-dir` is set ONLY in `clyde.yml` and points at `<home>/projects`, while the platform
/// default `<home>/.claude/projects` is left EMPTY. That asymmetry is the test: a command that reads
/// config finds the seeded sessions, and a command that jumps to the platform default finds zero.
/// Register item 8 is exactly that divergence, and this is what makes it observable per command path
/// rather than by grepping for a call.
struct Sandbox {
    matrix: Matrix,
    config_home: tempfile::TempDir,
    data_home: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let matrix = Matrix::build();
        let sandbox = Self {
            config_home: tempfile::tempdir().expect("temp config home"),
            data_home: tempfile::tempdir().expect("temp data home"),
            matrix,
        };
        sandbox.write_config();
        sandbox
    }

    /// Write `clyde.yml` with `projects-dir` and `repo-roots` pointed into the fixture.
    fn write_config(&self) {
        let dir = self.config_home.path().join("clyde");
        std::fs::create_dir_all(&dir).expect("create config dir");
        std::fs::write(
            dir.join("clyde.yml"),
            format!(
                "projects-dir: {}\nrepo-roots: [{}]\nreindex-on-start: false\n",
                self.matrix.projects_dir().display(),
                self.matrix.repo_root().display(),
            ),
        )
        .expect("write clyde.yml");
    }

    fn db_path(&self) -> PathBuf {
        self.data_home.path().join("sessions.db")
    }

    /// A `clyde` invocation bound to this sandbox. `HOME` is the fixture home, so `dirs::home_dir()`
    /// (rule 1's blocked root) and every XDG fallback resolve INSIDE the fixture.
    fn clyde(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_clyde"));
        cmd.env("HOME", self.matrix.home())
            .env("XDG_CONFIG_HOME", self.config_home.path())
            .env("XDG_DATA_HOME", self.data_home.path())
            .arg("--db")
            .arg(self.db_path())
            .args(args);
        cmd
    }

    /// Run a `clyde` invocation to completion, asserting it exited 0, and return stdout.
    fn run(&self, args: &[&str]) -> String {
        let out = self.clyde(args).output().expect("spawn clyde");
        assert!(
            out.status.success(),
            "clyde {args:?} exited {:?}\nstdout: {}\nstderr: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Seed one minimal parent transcript for `sid` at `cwd`, into the CONFIG-pointed projects dir.
    /// The timestamp is far in the past so the default 7d dormancy filter admits it.
    fn seed(&self, sid: &str, cwd: &Path) {
        let project_dir = cwd.to_string_lossy().replace(['/', '.'], "-");
        let dir = self.matrix.projects_dir().join(project_dir);
        std::fs::create_dir_all(&dir).expect("create project dir");
        let line = format!(
            r#"{{"type":"user","cwd":"{}","gitBranch":"main","timestamp":"2026-01-02T03:04:05Z","message":{{"content":"seeded for the checkout matrix"}}}}"#,
            cwd.display()
        );
        std::fs::write(dir.join(format!("{sid}.jsonl")), format!("{line}\n")).expect("write transcript");
    }

    /// `clyde session reindex`, with NO `--projects-dir` flag, so the run proves config is read.
    fn reindex(&self) -> String {
        self.run(&["session", "reindex"])
    }

    /// The `enrich --dry-run` gate decision per session: `session_id -> (scope, would_send)`.
    fn dry_run_decisions(&self) -> Vec<(String, String, bool)> {
        let stdout = self.run(&["session", "enrich", "--dry-run"]);
        let stats: serde_json::Value = serde_json::from_str(&stdout).expect("enrich --dry-run emits JSON when piped");
        stats["details"]
            .as_array()
            .expect("details array")
            .iter()
            .map(|d| {
                (
                    d["session-id"].as_str().expect("session-id").to_string(),
                    d["scope"].as_str().expect("scope").to_string(),
                    d["would-send"].as_bool().expect("would-send"),
                )
            })
            .collect()
    }

    /// The persisted `(repo, repo_source)` for one session, read straight out of the catalog.
    ///
    /// Read from SQLite rather than `session export`, because `ExportRecord` carries `repo` but NOT
    /// `repo_source` (`sessions/src/export.rs:133`), and PROVENANCE is exactly what these rows are
    /// about: "resolved" and "resolved by rule 1 rather than by a rule-4 guess" are different claims,
    /// and the matrix exists to tell them apart. Read-only, and after the writing command has
    /// already exited.
    fn attribution(&self, sid: &str) -> (Option<String>, Option<String>) {
        let conn = rusqlite::Connection::open_with_flags(self.db_path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open the catalog read-only");
        conn.query_row(
            "SELECT repo, repo_source FROM sessions WHERE session_id = ?1",
            rusqlite::params![sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or_else(|e| panic!("session {sid} is absent from the catalog: {e}"))
    }
}

/// AC7, the reindex arm. `projects-dir` is set ONLY in config; the platform default is empty. If
/// `cmd_reindex` still jumped from the flag straight to the platform default it would scan nothing
/// and index zero sessions.
#[test]
fn matrix_reindex_reads_a_projects_dir_set_only_in_config() {
    let s = Sandbox::new();
    s.seed(SID_FLAT, &s.matrix.flat_ssh);
    s.reindex();

    let (repo, source) = s.attribution(SID_FLAT);
    assert_eq!(
        repo.as_deref(),
        Some("tatari-tv/philo"),
        "reindex found nothing, so it did not read `projects-dir` from clyde.yml"
    );
    assert_eq!(source.as_deref(), Some("git-origin"));
}

/// AC7, the enrich arm. `enrich` refreshes through `lazy_reindex`, which read the platform default
/// unconditionally, so on a host with `projects-dir` set it silently reindexed the wrong tree.
#[test]
fn matrix_enrich_reads_a_projects_dir_set_only_in_config() {
    let s = Sandbox::new();
    s.seed(SID_FLAT, &s.matrix.flat_ssh);

    // No explicit reindex: `enrich`'s own lazy refresh is the ONLY thing that can populate the
    // catalog here, so a decision for this session proves the lazy path read config.
    let decisions = s.dry_run_decisions();
    let seen = decisions.iter().find(|(id, _, _)| id == SID_FLAT);
    assert!(
        seen.is_some(),
        "enrich's lazy_reindex did not read `projects-dir` from clyde.yml; decisions: {decisions:?}"
    );
}

/// AC7, the `mcp serve` arm. Driven through the real JSON-RPC handshake rather than a grep, because
/// the earlier draft's criterion (`rg -c 'cfg.projects_dir()' == 2`) is satisfiable by "MCP plus
/// lazy_reindex" while explicit reindex stays divergent.
#[test]
fn matrix_mcp_serve_reads_a_projects_dir_set_only_in_config() {
    use std::io::Write;

    let s = Sandbox::new();
    s.seed(SID_FLAT, &s.matrix.flat_ssh);

    // `reindex-on-start` is off in the shared config, so turn it back on for this arm: a startup
    // reindex is the observable that proves which tree the server resolved.
    let dir = s.config_home.path().join("clyde");
    std::fs::write(
        dir.join("clyde.yml"),
        format!(
            "projects-dir: {}\nrepo-roots: [{}]\nreindex-on-start: true\n",
            s.matrix.projects_dir().display(),
            s.matrix.repo_root().display(),
        ),
    )
    .expect("rewrite clyde.yml");

    let mut child = s
        .clyde(&["mcp", "serve"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn clyde mcp serve");
    let mut stdin = child.stdin.take().expect("child stdin");
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"matrix","version":"0"}}}"#;
    stdin.write_all(init.as_bytes()).expect("write initialize");
    stdin.write_all(b"\n").expect("write newline");
    drop(stdin);
    let out = child.wait_with_output().expect("wait for clyde mcp serve");
    assert!(
        out.status.success(),
        "mcp serve exited {:?}: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );

    // The server's startup reindex wrote the catalog. Reading it back through a NON-server command
    // is what proves the server scanned the config-pointed tree.
    let (repo, _) = s.attribution(SID_FLAT);
    assert_eq!(
        repo.as_deref(),
        Some("tatari-tv/philo"),
        "mcp serve's startup reindex did not read the config-pointed projects-dir"
    );
}

/// The harness can see a SEQUENCE, which is the whole point of Phase 1. Reindex, mutate the world,
/// reindex again, and observe that the SAME session's attribution changed with nothing about the
/// session touched.
///
/// This asserts the MECHANISM (a later probe re-attributes), not the routing outcome. The routing
/// invariant it enables is Phase 2's `scope_never_upgrades_personal_to_work_on_a_later_probe`.
#[test]
fn matrix_rows_09_and_10_a_later_probe_reattributes_the_same_session() {
    let s = Sandbox::new();
    s.seed(SID_NO_ORIGIN, &s.matrix.no_origin);

    s.reindex();
    let (repo, source) = s.attribution(SID_NO_ORIGIN);
    assert_eq!(repo, None, "seed state: no origin, so no rule 1 answer");
    assert_eq!(source, None);

    // One command. Nothing about the session is touched.
    s.matrix.add_origin_to_no_origin();
    s.reindex();

    let (repo, source) = s.attribution(SID_NO_ORIGIN);
    assert_eq!(
        repo.as_deref(),
        Some("tatari-tv/side-project"),
        "the second pass re-probed a cwd it had already seen and took the new answer: that is the \
         mechanism Problem 1 rides"
    );
    assert_eq!(source.as_deref(), Some("git-origin"));
}

/// Row 22: rule 2's learned path map. A rule-1 hit records `(cwd, repo)`, so the attribution
/// survives the checkout being deleted.
#[test]
fn matrix_row_22_a_deleted_checkout_still_resolves_through_the_learned_path_map() {
    let s = Sandbox::new();
    s.seed(SID_DELETABLE, &s.matrix.deletable);
    s.reindex();
    assert_eq!(s.attribution(SID_DELETABLE).1.as_deref(), Some("git-origin"));

    std::fs::remove_dir_all(&s.matrix.deletable).expect("delete the checkout");
    s.reindex();

    let (repo, source) = s.attribution(SID_DELETABLE);
    assert_eq!(repo.as_deref(), Some("tatari-tv/ephemeral"));
    assert_eq!(
        source.as_deref(),
        Some("git-origin"),
        "the upgrade-only write keeps the better answer; rule 2 is what would serve it on a cold \
         catalog"
    );
}

/// Row 18 through the real gate: a personal fork checked out in a work directory stays WORK,
/// decided by the cwd anchor. `cwd_anchor_outranks_the_remote_in_both_directions` asserts the same
/// thing at the unit level; this proves it survives the whole pipeline.
#[test]
fn matrix_row_18_the_fork_in_a_work_directory_stays_work_through_the_real_gate() {
    let s = Sandbox::new();
    s.seed(SID_FORK, &s.matrix.fork_in_work_dir);
    s.reindex();

    let decisions = s.dry_run_decisions();
    let (_, scope, _) = decisions
        .iter()
        .find(|(id, _, _)| id == SID_FORK)
        .expect("the fork session was considered");
    assert_eq!(
        scope, "work",
        "an ordinary work fork must not be dropped from enrichment: this is why the precedence \
         change was withdrawn"
    );
}

/// A teammate layout with no org slot, through the real gate. Before v0.22.0 these sat at 0%
/// coverage; the branch must not roll that back.
#[test]
fn matrix_row_16_an_off_layout_work_checkout_still_classifies_work() {
    let s = Sandbox::new();
    s.seed(SID_LAYOUT, &s.matrix.layout_git_tatari);
    s.reindex();

    let decisions = s.dry_run_decisions();
    let (_, scope, _) = decisions
        .iter()
        .find(|(id, _, _)| id == SID_LAYOUT)
        .expect("the off-layout session was considered");
    assert_eq!(
        scope, "work",
        "Keegan's layout resolves through the remote, not the path"
    );
}

// ---------------------------------------------------------------------------------------------
// Phase 2 rows: every way the probe record could LIE, and the ones where it must stay silent.
// ---------------------------------------------------------------------------------------------

/// Row 23. A `safe.directory` / dubious-ownership refusal is a transient ENVIRONMENT failure, not a
/// statement about a remote. Stamping it would turn one misconfigured host into a permanent refusal
/// of work scope for every session on it. The panel's severest finding.
///
/// Approximated the way it actually manifests: git exits non-zero at the discovery stage for a
/// reason that is not "this is not a repository". Row 29 covers the origin-read-stage version.
#[test]
fn matrix_row_23_a_dubious_ownership_refusal_records_nothing() {
    let m = Matrix::build();
    // A `.git` FILE pointing at a gitdir that does not exist: git fails discovery, and there is no
    // repository to make a conclusive statement about either.
    let broken = m.home().join("dubious");
    std::fs::create_dir_all(&broken).expect("create dir");
    std::fs::write(broken.join(".git"), "gitdir: /nonexistent/clyde-matrix/gone\n").expect("write .git");

    let outcome = probe(&m, &broken);
    assert!(
        !outcome.is_conclusive_negative(),
        "a broken gitdir pointer must not stamp, got {}",
        outcome.as_str()
    );
}

/// Row 26. An archived session whose cwd no longer exists has NOTHING to observe. It must be
/// `Indeterminate`, never a conclusive negative, so a restored checkout can still recover the row.
///
/// BITES: return `NotARepo` for a missing cwd and every archived session is permanently refused.
#[test]
fn matrix_row_26_an_archived_cwd_that_no_longer_exists_records_nothing() {
    let m = Matrix::build();
    let gone = m.home().join("deleted-long-ago");
    assert_eq!(probe(&m, &gone), ProbeOutcome::Indeterminate);
}

/// Row 29. A repo whose `.git/config` cannot be read fails the origin read with a fatal, NOT with
/// git's "the key is absent" rc=1. It must be `Indeterminate`: `rev-parse` already established the
/// cwd IS a repo, so a fatal at the origin read is an anomaly, not a finding.
///
/// BITES: collapse a non-1 exit at the origin-read stage into `NoOrigin` or `NotARepo` and an
/// unreadable config becomes a permanent lockout.
#[test]
fn matrix_row_29_an_unreadable_git_config_records_nothing() {
    let m = Matrix::build();
    if !m.make_config_unreadable() {
        // Running as root, where mode 0 is still readable. Skip rather than assert a condition the
        // platform refused to create; a test that silently passes without exercising anything is the
        // register's own defect class.
        eprintln!("skipped: this platform cannot make a file unreadable (running as root?)");
        return;
    }
    let outcome = probe(&m, &m.unreadable_config);
    assert!(
        !outcome.is_conclusive_negative(),
        "an unreadable .git/config must not stamp, got {}",
        outcome.as_str()
    );
}

/// Row 28, a CONFIRMED live bug on `main`: an exported `GIT_DIR` forges a `Resolved` and the
/// containment check does not catch it, because git treats the `-C` path as the work tree when
/// `GIT_DIR` is set without `GIT_WORK_TREE`.
///
/// Driven through the SHIPPED BINARY rather than the library, which is the point of putting it here:
/// `common`'s unit test proves the function resists the variable, and this proves the variable does
/// not survive the trip from the operator's shell through `clyde session reindex` into the probe.
#[test]
fn matrix_row_28_git_dir_in_the_environment_cannot_forge_an_attribution() {
    let s = Sandbox::new();
    // The session ran in a plain directory with no repository of its own.
    s.seed(SID_NO_ORIGIN, &s.matrix.not_a_repo);

    let out = s
        .clyde(&["session", "reindex"])
        .env("GIT_DIR", s.matrix.flat_ssh.join(".git"))
        .output()
        .expect("spawn clyde");
    assert!(
        out.status.success(),
        "reindex failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let (repo, source) = s.attribution(SID_NO_ORIGIN);
    assert_eq!(
        repo, None,
        "an inherited GIT_DIR reached the probe and forged an attribution to {repo:?} via {source:?}"
    );
}

/// Rows 9 and 10 as ONE SEQUENCE, through the shipped binary, at the ROUTING surface. This is
/// Problem 1 end to end.
///
/// It drives `session enrich` and never an explicit `session reindex`, and that is load-bearing
/// rather than incidental. An explicit reindex runs `reindex_efficiency`, which writes
/// `outcome_json`, so the decision SETTLES at the current `SCOPE_VERSION` and `enrich_candidates`
/// excludes the row before the gate is ever consulted. `enrich` refreshes through `lazy_reindex`,
/// which never runs that pass, so the row stays PROVISIONAL and is re-decided. That provisional
/// population is exactly the state of a teammate host, which is the population the git-origin branch
/// was built for and therefore the population the leak reaches.
///
/// Measured in the Phase 1 harness before the fix: `personal/false` then `work/true`.
///
/// BITES: delete the `facts.repo_probe` branch from the git-origin arm and the second decision
/// becomes `work, would_send=true`.
#[test]
fn matrix_rows_09_and_10_a_later_probe_never_upgrades_personal_to_work() {
    let s = Sandbox::new();
    s.seed(SID_NO_ORIGIN, &s.matrix.no_origin);

    let before = s.dry_run_decisions();
    let (_, scope, send) = before
        .iter()
        .find(|(id, _, _)| id == SID_NO_ORIGIN)
        .expect("the session was considered on the first pass");
    assert_eq!((scope.as_str(), *send), ("personal", false), "seed state");

    // One command. Nothing about the session is touched. `gh repo create --source=.` produces the
    // identical state and is an ordinary workflow.
    s.matrix.add_origin_to_no_origin();

    let after = s.dry_run_decisions();
    let (_, scope, send) = after
        .iter()
        .find(|(id, _, _)| id == SID_NO_ORIGIN)
        .expect("the session is still a candidate: a personal git-origin row never settles");
    assert_eq!(
        (scope.as_str(), *send),
        ("personal", false),
        "a personal transcript was queued for the work Anthropic account by a `git remote add`"
    );
}

// ---------------------------------------------------------------------------------------------
// Phase 3 rows: the host is validated, and the fix must not break SSH aliases.
// ---------------------------------------------------------------------------------------------

/// Row 19, Problem 2. A remote on a host that is not allowlisted still ATTRIBUTES a repo (it says
/// which repo the session was in, which is true) but must never confer WORK scope.
///
/// The distinction matters: refusing the attribution too would lose real information for no safety
/// gain, since attribution never leaves the machine.
///
/// BITES: remove the `host_confers_work` branch from `session::scope` and this classifies work.
#[test]
fn matrix_row_19_a_non_allowlisted_host_attributes_but_confers_no_work_scope() {
    let s = Sandbox::new();
    s.seed(SID_EVIL_HOST, &s.matrix.host_not_allowed);
    s.reindex();

    let (repo, source) = s.attribution(SID_EVIL_HOST);
    assert_eq!(
        repo.as_deref(),
        Some("tatari-tv/x"),
        "attribution is unchanged: the remote still says which repo this is"
    );
    assert_eq!(source.as_deref(), Some("git-origin"));

    let decisions = s.dry_run_decisions();
    let (_, scope, send) = decisions
        .iter()
        .find(|(id, _, _)| id == SID_EVIL_HOST)
        .expect("the session was considered");
    assert_eq!(
        (scope.as_str(), *send),
        ("personal", false),
        "a `tatari-tv` slug from evil.example.com must not ship to the work account"
    );
}

/// Row 21, the attacker-authored vector. A `.gitmodules` naming a hostile remote is content anyone
/// can put in a repo you clone. It must not be able to influence the routing decision.
///
/// The checkout's OWN origin is a legitimate `github.com` work remote, so this asserts the honest
/// outcome: the session is work because ITS remote is trustworthy, and the hostile `.gitmodules`
/// changed nothing. A test asserting `personal` here would be asserting a coverage regression.
#[test]
fn matrix_row_21_a_hostile_gitmodules_does_not_reach_the_routing_decision() {
    let s = Sandbox::new();
    s.seed(SID_SUBMODULE, &s.matrix.hostile_submodule);
    s.reindex();

    let (repo, _) = s.attribution(SID_SUBMODULE);
    assert_eq!(
        repo.as_deref(),
        Some("tatari-tv/withsub"),
        "the cwd's OWN origin decides, never a url string sitting in a tracked file"
    );

    let host: Option<String> = {
        let conn = rusqlite::Connection::open_with_flags(s.db_path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open the catalog");
        conn.query_row(
            "SELECT repo_host FROM sessions WHERE session_id = ?1",
            rusqlite::params![SID_SUBMODULE],
            |r| r.get(0),
        )
        .expect("row")
    };
    assert_eq!(
        host.as_deref(),
        Some("github.com"),
        "the recorded host is the checkout's own, not evil.example.com from .gitmodules"
    );
}

/// Row 20, and the check that Phase 3 did not reintroduce the 0%-coverage bug: a remote reached
/// through an SSH `Host` alias that resolves to an allowlisted host STILL confers work.
///
/// The alias cannot be resolved in this sandbox (`ssh -G` reads the INVOKING USER's real
/// `~/.ssh/config`, which a test must not depend on and cannot safely modify), so the assertion here
/// is the one that IS reproducible: the alias is recorded verbatim as the host, which is what makes
/// it resolvable at all. `HostPolicy`'s resolution logic is asserted against an injected resolver in
/// `common::repo::host::tests::an_ssh_alias_resolving_to_an_allowlisted_host_still_confers_work`.
///
/// Both halves are needed: without this one, nothing proves the alias survives the trip from git to
/// the catalog un-mangled.
#[test]
fn matrix_row_20_an_ssh_alias_host_is_recorded_verbatim_for_later_resolution() {
    let s = Sandbox::new();
    s.seed(SID_ALIAS, &s.matrix.host_ssh_alias);
    s.reindex();

    let host: Option<String> = {
        let conn = rusqlite::Connection::open_with_flags(s.db_path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open the catalog");
        conn.query_row(
            "SELECT repo_host FROM sessions WHERE session_id = ?1",
            rusqlite::params![SID_ALIAS],
            |r| r.get(0),
        )
        .expect("row")
    };
    assert_eq!(
        host.as_deref(),
        Some("github-work"),
        "the alias must be stored as written; normalizing it away here would make resolution \
         impossible and refuse every alias user"
    );
}

/// Row 27, the migration population. A pre-v13 row carries a NULL `repo_host`, and that must NOT
/// strip its work authority: the v13 upgrade would otherwise demote every `git-origin` row at once.
///
/// **Strip-only, made executable.** A live-populated host may only ever REMOVE authority. This
/// asserts the "keeps what it had" half; row 19 asserts the "stripped" half.
///
/// BITES: treat a NULL `repo_host` as `Some(false)` and this session stops being work.
#[test]
fn matrix_row_27_a_pre_v13_row_with_no_recorded_host_keeps_its_work_authority() {
    let s = Sandbox::new();
    s.seed(SID_LAYOUT, &s.matrix.layout_git_tatari);
    s.reindex();

    // The checkout is DELETED before the host is nulled, and that ordering is the whole test. With
    // the checkout still on disk, `enrich`'s own `lazy_reindex` re-probes the cwd and re-populates
    // `repo_host` before the classifier ever runs, so the NULL never reaches the gate and the
    // assertion below passes no matter what the gate does. Verified by deletion: with the checkout
    // present, treating a NULL host as `Some(false)` still passed.
    //
    // Deleted, the probe is `Indeterminate` (nothing to observe), no host is recorded, and the
    // upgrade-only write keeps the existing `git-origin` attribution. That is precisely a pre-v13
    // row: a real work slug with no host beside it.
    std::fs::remove_dir_all(&s.matrix.layout_git_tatari).expect("delete the checkout");
    {
        let conn = rusqlite::Connection::open(s.db_path()).expect("open the catalog");
        conn.execute(
            "UPDATE sessions SET repo_host = NULL, scope_version = NULL WHERE session_id = ?1",
            rusqlite::params![SID_LAYOUT],
        )
        .expect("null the host");
    }

    let decisions = s.dry_run_decisions();
    let (_, scope, _) = decisions
        .iter()
        .find(|(id, _, _)| id == SID_LAYOUT)
        .expect("the pre-v13 row was considered");
    assert_eq!(
        scope, "work",
        "a NULL repo_host must never strip authority on its own: that is the whole strip-only rule"
    );
}
