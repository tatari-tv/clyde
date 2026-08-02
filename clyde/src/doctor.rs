//! `clyde doctor`: health-check the migration and the live integrations. Reports the resolved
//! clyde data/config/cache locations, what each integration currently points at, and the permit
//! events-DB presence + row count. Exits NON-ZERO while any integration still resolves to an old
//! binary name (`klod`/`ccu`/`claude-permit`) or any tool's state still lives only at a legacy
//! path -- so a missed `bootstrap` step fails loud.

use std::path::{Path, PathBuf};

use colored::Colorize;
use eyre::{Context, Result};
use log::debug;

use crate::bootstrap::Paths;

/// Entry point for `clyde doctor`. Returns the intended process exit code (0 healthy, 1 if any
/// legacy target/state remains).
///
/// `db_path` is the catalog to report attribution and routing from. Those lines do not WRITE any
/// attribution or routing state, but this is not a read-only command: `sessions::Db::open_at` runs
/// `Db::init`, so opening the catalog here applies the snapshot helpers and the full migration
/// ladder. On a host still at v12, `clyde doctor` therefore writes `<db>.pre-v13.bak` and advances
/// `user_version` to 13. That is column-add only and snapshotted first, so it is safe, but it is a
/// side effect and calling it read-only was wrong.
///
/// The attribution lines deliberately do NOT feed [`Report::healthy`]: a session refused by the routing gate is the gate
/// working, not a broken installation, and a diagnostic that exits non-zero for correct behavior is
/// one people stop running.
pub fn run(db_path: &Path) -> Result<i32> {
    debug!("doctor::run: db={}", db_path.display());
    let paths = Paths::from_env()?;
    let report = diagnose(&paths)?;
    print_report(&paths, &report);
    // Config and catalog are both best-effort here. `doctor` exists to tell an operator what is
    // wrong, so it must still print everything it CAN when one source is unreadable, and say which
    // one failed rather than dying on it.
    match attribution(db_path) {
        Ok(Some(section)) => print_attribution(&section),
        Ok(None) => println!("\n  {} no catalog at {}", "attribution:".bold(), db_path.display()),
        Err(e) => println!("\n  {} could not read the catalog: {e}", "attribution:".red()),
    }
    Ok(if report.healthy() { 0 } else { 1 })
}

/// What `clyde doctor` reports about attribution and routing.
struct Attribution {
    /// The resolved config file path, or `None` when no config file exists.
    config_path: Option<PathBuf>,
    /// One entry per effective `repo-roots` entry, in configured order. Reported PER ROOT rather
    /// than folded into a single verdict: with two roots, a summary line saying "exists" cannot say
    /// WHICH one exists, and a teammate whose second root is typo'd would read a healthy line.
    repo_roots: Vec<RepoRootState>,
    work_remote_hosts: Vec<String>,
    routing: sessions::RoutingSummary,
}

/// One configured root's own diagnosis. Every field is about THAT root; nothing here is a rollup.
struct RepoRootState {
    path: PathBuf,
    exists: bool,
    /// Whether this root contains at least one `<org>/<repo>` pair. When it does not, rule 4 is
    /// INERT for this root and says so, rather than looking like it is simply not firing.
    has_org_repo: bool,
}

/// Read the attribution picture, or `Ok(None)` when there is no catalog to read.
fn attribution(db_path: &Path) -> Result<Option<Attribution>> {
    if !db_path.exists() {
        return Ok(None);
    }
    let cfg = common::config::load().context("failed to load clyde config")?;
    let db = sessions::Db::open_at(db_path)?;
    let repo_roots: Vec<RepoRootState> = cfg
        .repo_roots()
        .iter()
        .map(|path| RepoRootState {
            exists: path.is_dir(),
            has_org_repo: has_org_repo_pair(path),
            path: path.clone(),
        })
        .collect();
    Ok(Some(Attribution {
        config_path: common::config::config_file_path().filter(|p| p.exists()),
        repo_roots,
        work_remote_hosts: cfg.work_remote_hosts().to_vec(),
        routing: db.routing_summary(&session::Anchors::new(cfg.repo_roots()), cfg.work_remote_hosts())?,
    }))
}

/// Whether `root` holds at least one `<org>/<repo>` directory pair, which is the shape rule 4
/// matches. Two `read_dir` levels, stopping at the first hit.
fn has_org_repo_pair(root: &Path) -> bool {
    let Ok(orgs) = std::fs::read_dir(root) else {
        return false;
    };
    for org in orgs.filter_map(std::result::Result::ok) {
        if !org.path().is_dir() {
            continue;
        }
        if let Ok(mut repos) = std::fs::read_dir(org.path())
            && repos.any(|r| r.is_ok_and(|r| r.path().is_dir()))
        {
            return true;
        }
    }
    false
}

fn print_attribution(a: &Attribution) {
    println!("\n{}", "attribution".bold());
    match &a.config_path {
        Some(path) => println!("  config:        {}", path.display()),
        None => println!("  config:        {} (all defaults)", "none".dimmed()),
    }
    // One line per root, not one line for "the root". A folded verdict cannot name WHICH of two
    // roots is missing, which is the whole reason the key became a list.
    for (i, root) in a.repo_roots.iter().enumerate() {
        let label = if i == 0 { "  repo-roots:  " } else { "               " };
        let state = if root.exists {
            String::new()
        } else {
            format!(" {}", "(does not exist)".red())
        };
        println!("{label}  {}{}", root.path.display(), state);
        if root.exists && !root.has_org_repo {
            // Not a failure: plenty of hosts have no `<org>/<repo>` layout at all. But rule 4
            // silently never firing looks identical to rule 4 being broken, and this is the
            // difference. Per root, because one root can be inert while another is not.
            println!(
                "    {} no <org>/<repo> pair under this root, so rule 4 (path-guess) is INERT for it",
                "note:".yellow()
            );
        }
    }
    if a.work_remote_hosts.is_empty() {
        // An empty allowlist is fail-closed at the GATE (`HostPolicy::confers_work` matches nothing,
        // so every recorded host reads `Some(false)` and refuses a work slug), and `routing_summary`
        // now runs that same policy. Printing a blank line next to a large `host-refused` decision
        // count would give the operator the symptom with no cause.
        println!(
            "  work hosts:    {} {}",
            "none".red(),
            "every recorded host is refused; set `work-remote-hosts` in clyde.yml".dimmed()
        );
    } else {
        println!("  work hosts:    {}", a.work_remote_hosts.join(", "));
    }

    println!("  resolved by:");
    for (source, n) in &a.routing.by_source {
        println!("    {source:<14} {n}");
    }

    // TWO groups, because these are two different kinds of claim and printing them as one list is
    // what made `probe-refused` read as 326 refusals when the number of decisions a probe refusal
    // made was 0. A DECISION count says "this is what decided the row"; a CONDITION count says "this
    // fact is present on the row, and it decided nothing on its own".
    let r = &a.routing;

    // Decisions, in the classifier's own precedence order, so the list reads top-down the way a
    // decision is actually made. Each count comes from running the classifier over the catalog, so
    // it cannot drift from the enrich gate. The group SUMS to the catalog row count.
    println!("  routing decisions:");
    for (label, n, remedy) in r.basis_counts() {
        println!("    {label:<14} {n:<6} {}", remedy.dimmed());
    }
    println!(
        "    {:<14} {:<6} {}",
        "(total)",
        r.decisions_total(),
        "sums to the catalog row count; every row is decided by exactly one basis".dimmed()
    );

    // Conditions. Each line still carries its own REMEDY: at 3am a count is not actionable on its
    // own, and these have different fixes, which is why they are separate counts and not one.
    println!("  routing conditions:");
    for (label, n, remedy) in [
        (
            "probe-recorded",
            r.probe_recorded,
            "rows carrying a conclusive negative; clear a stale one with `session reindex --clear-probe --session <id>`",
        ),
        (
            "host-unknown",
            r.host_unknown,
            "indexed before v13; keeps pre-v13 authority until a reprobe records a host",
        ),
        (
            "anchor/remote",
            r.anchor_remote_disagreement,
            "cwd and remote disagree: an ordinary fork, or a personal clone under the work org",
        ),
    ] {
        println!("    {label:<14} {n:<6} {}", remedy.dimmed());
    }

    // The LIVE half. `Blocked`, `OutsideRoot` and `Indeterminate` all record nothing (that is what
    // keeps a transient failure from becoming a lockout), so the catalog cannot say which of them a
    // row hit. `doctor` re-probes, which is exactly the right place for it: this is a question about
    // the machine as it is NOW, not about when a session ran.
    //
    // SAMPLED, and it says so. This used to claim "memoized per repository, so it is a handful of
    // `git` calls" -- which is false: `reprobe_candidates` is `SELECT DISTINCT cwd`, so every entry
    // is a distinct memo key and the memo never hits. Measured candidate counts on live hosts are in
    // the hundreds, so the unbounded form spawned hundreds of `git` processes serially, with no
    // progress output, every time an operator ran `doctor` because something was already wrong.
    let sampled = a.routing.reprobe_candidates.len().min(REPROBE_SAMPLE_MAX);
    let (blocked, outside, indeterminate) = reprobe(&a.routing.reprobe_candidates);
    if a.routing.reprobe_candidates.len() > REPROBE_SAMPLE_MAX {
        println!(
            "    {:<14} {:<6} {}",
            "(sampled)",
            format!("{sampled}/{}", a.routing.reprobe_candidates.len()),
            "the three counts below are a sample; each one costs its own `git` call".dimmed()
        );
    }
    println!(
        "    {:<14} {:<6} {}",
        "blocked",
        blocked,
        "cwd resolves to a blocked root ($HOME); correct, and never attributed".dimmed()
    );
    println!(
        "    {:<14} {:<6} {}",
        "outside-root",
        outside,
        "git found a repo that does not contain the cwd; nothing is recorded for these".dimmed()
    );
    println!(
        "    {:<14} {:<6} {}",
        "indeterminate",
        indeterminate,
        "git answered NOTHING; check `safe.directory` and that git is installed".dimmed()
    );
    if indeterminate > 0 && indeterminate == sampled {
        println!(
            "      {} EVERY probe on this host is indeterminate, which is a git problem rather than \
             a layout one",
            "warning:".yellow()
        );
    }
}

/// The reprobe sample cap. Every candidate is a DISTINCT cwd, so each one costs its own `git`
/// invocation and the resolver's memo cannot amortize them. A few hundred serial spawns is not what
/// a diagnostic should cost, so `doctor` samples and says that it sampled.
const REPROBE_SAMPLE_MAX: usize = 64;

/// Re-probe each cwd and tally which non-recording outcome it hits: `(blocked, outside, indeterminate)`.
fn reprobe(cwds: &[String]) -> (usize, usize, usize) {
    use common::repo::ProbeOutcome;
    let resolver = common::repo::SharedResolver::new();
    let (mut blocked, mut outside, mut indeterminate) = (0, 0, 0);
    for cwd in cwds.iter().take(REPROBE_SAMPLE_MAX) {
        match resolver.probe(Path::new(cwd)) {
            ProbeOutcome::Blocked => blocked += 1,
            ProbeOutcome::OutsideRoot => outside += 1,
            ProbeOutcome::Indeterminate => indeterminate += 1,
            // Resolved or conclusive: the catalog is simply behind a reindex, which is not a fault.
            _ => {}
        }
    }
    debug!("doctor::reprobe: blocked={blocked} outside={outside} indeterminate={indeterminate}");
    (blocked, outside, indeterminate)
}

/// Where a given integration currently resolves.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// Points at the clyde umbrella form (healthy).
    Clyde,
    /// Still points at the old standalone binary (unhealthy).
    Legacy(&'static str),
    /// Not present at all (not an error -- nothing to repoint).
    Absent,
}

impl Target {
    fn is_legacy(&self) -> bool {
        matches!(self, Target::Legacy(_))
    }
    fn label(&self) -> String {
        match self {
            Target::Clyde => "clyde".green().to_string(),
            Target::Legacy(name) => format!("{} (legacy)", name).red().to_string(),
            Target::Absent => "absent".dimmed().to_string(),
        }
    }
}

/// The full health picture.
#[derive(Debug)]
pub struct Report {
    pub statusline: Target,
    pub hook_global: Target,
    pub hook_local: Target,
    pub timer: Target,
    /// The active enrich unit name (clyde-enrich.service / klod-enrich.service), if any.
    pub timer_unit: Option<String>,
    /// The active enrich unit's `ExecStart=` line, if readable.
    pub timer_execstart: Option<String>,
    /// State of the reindex sweep's timer: `Clyde` when its service, timer, and `timers.target.wants`
    /// enable symlink are all present, `Absent` otherwise.
    ///
    /// Reported because the reindex sweep is what STAGES dormant transcripts before Claude Code's TTL
    /// reaps them, and an absent timer means that race is silently open: sessions keep aging into the
    /// permanently-unpriceable state with nothing in any output saying so. The timer only exists on a
    /// host where `clyde bootstrap --install-timer` ran, so this check is what makes its absence
    /// VISIBLE rather than silent. Never `Legacy`: the unit is new, so no pre-rename spelling exists.
    pub reindex_timer: Target,
    pub events_db_at_clyde: bool,
    pub events_db_at_legacy: bool,
    pub events_db_rows: Option<i64>,
    /// Any tool state still living at a legacy path (klod data/config dirs, ccu/cr/claude-permit
    /// config, cr/ccu pricing override), by label.
    pub legacy_state: Vec<String>,
    /// Where each tool's log currently lives, unified under `<xdg-data>/clyde/logs/<tool>.log`
    /// (Phase 8, D3). Always populated (the target location, whether or not the file has been
    /// written yet) so `clyde doctor` is a one-stop answer to "where are the logs".
    pub log_locations: Vec<(&'static str, PathBuf)>,
    /// Legacy per-tool log dirs still present on disk (`ccu/logs/`, `claude-permit/logs/`,
    /// `claude-report/logs/`). Purely informational: logs are disposable diagnostics, so this
    /// does NOT feed [`Report::healthy`] the way `legacy_state` does.
    pub legacy_log_dirs: Vec<PathBuf>,
}

impl Report {
    /// Healthy iff no integration is legacy, no events DB is stranded at the legacy path, and no
    /// tool state lives at a legacy path.
    pub fn healthy(&self) -> bool {
        !self.statusline.is_legacy()
            && !self.hook_global.is_legacy()
            && !self.hook_local.is_legacy()
            && !self.timer.is_legacy()
            && !self.events_db_at_legacy
            && self.legacy_state.is_empty()
    }

    /// Whether the unhealthy state is specifically pre-rename `klod` residue, which `clyde bootstrap`
    /// no longer migrates (design Phase 4). A NAMED discriminator because [`print_report`] otherwise
    /// has only [`Report::healthy`], a bool, and would print a remedy that cannot work. Every other
    /// unhealthy cause (`ccu`, `claude-permit`, a drifted enrich unit) IS still fixed by `bootstrap`.
    pub fn has_klod_residue(&self) -> bool {
        self.timer == Target::Legacy("klod") || self.legacy_state.iter().any(|s| s.contains("klod"))
    }
}

/// Compute the health picture from the filesystem under `paths`. Pure read-only (no systemctl).
pub fn diagnose(paths: &Paths) -> Result<Report> {
    let statusline = statusline_target(paths);
    let hook_global = hook_target(&paths.home.join(".claude").join("settings.json"));
    let hook_local = hook_target(&paths.home.join(".claude").join("settings.local.json"));
    let (timer, timer_unit, timer_execstart) = timer_state(paths);
    let reindex_timer = reindex_timer_state(paths);

    let clyde_db = paths.clyde_events_db();
    let legacy_db = paths.xdg_data.join("claude-permit").join("events.db");
    let events_db_at_clyde = clyde_db.exists();
    let events_db_at_legacy = legacy_db.exists();
    let events_db_rows = if events_db_at_clyde { count_events(&clyde_db).ok() } else { None };

    let mut legacy_state = Vec::new();
    // Per-tool config still living only at a legacy path.
    check_legacy_only(
        "cost config (ccu/ccu.yml)",
        &paths.xdg_config.join("ccu").join("ccu.yml"),
        &paths.xdg_config.join("clyde").join("cost.yml"),
        &mut legacy_state,
    );
    if permit_legacy_config_present(paths) && !paths.xdg_config.join("clyde").join("permit.yml").exists() {
        legacy_state.push("permit config (claude-permit/)".to_string());
    }
    let clyde_pricing = paths.xdg_config.join("clyde").join("pricing.json");
    if (paths.xdg_config.join("cr").join("pricing.json").exists()
        || paths.xdg_config.join("ccu").join("pricing.json").exists())
        && !clyde_pricing.exists()
    {
        legacy_state.push("pricing override (cr/ccu)".to_string());
    }
    // Legacy klod data/config dirs should be gone (merged into clyde) after bootstrap.
    if paths.xdg_data.join("klod").exists() {
        legacy_state.push("klod data dir".to_string());
    }
    if paths.xdg_config.join("klod").exists() {
        legacy_state.push("klod config dir".to_string());
    }
    // The timer half of the same tripwire, naming each residue path. `clyde bootstrap` no longer
    // migrates any of this (design Phase 4), so `doctor` is the ONE channel that tells such a host the
    // truth, and its remedy branches accordingly in `print_report`.
    legacy_state.extend(legacy_timer_residue(paths));
    // An enrich unit still REFERRING to a credential clyde no longer reads. Reported through
    // `legacy_state` rather than as its own `Report` field because that channel already feeds
    // `healthy()` and already prints one line per item with the correct `run \`clyde bootstrap\``
    // remedy -- and `bootstrap` genuinely repairs this one (`refresh_clyde_unit` converges the unit on
    // the canonical body). Cosmetic on its face, but the file states a falsehood about a credential,
    // which is exactly the class that must fail loud rather than sit unnoticed.
    let clyde_svc = paths.clyde_unit();
    if std::fs::read_to_string(&clyde_svc)
        .ok()
        .is_some_and(|text| crate::bootstrap::mentions_retired_credential(&text))
    {
        legacy_state.push("enrich unit references a retired credential (clyde-enrich.service)".to_string());
    }

    let (log_locations, legacy_log_dirs) = log_state(paths);

    Ok(Report {
        statusline,
        hook_global,
        hook_local,
        timer,
        timer_unit,
        timer_execstart,
        reindex_timer,
        events_db_at_clyde,
        events_db_at_legacy,
        events_db_rows,
        legacy_state,
        log_locations,
        legacy_log_dirs,
    })
}

/// Per-tool log locations under the unified `clyde/logs/` dir (Phase 8, D3), plus any legacy
/// per-tool log dirs still present. Informational only -- legacy logs are disposable diagnostics,
/// not migration state, so callers must NOT fold `legacy_log_dirs` into [`Report::healthy`].
fn log_state(paths: &Paths) -> (Vec<(&'static str, PathBuf)>, Vec<PathBuf>) {
    let unified_dir = paths.xdg_data.join("clyde").join("logs");
    let log_locations = vec![
        ("clyde", unified_dir.join("clyde.log")),
        ("cost", unified_dir.join("cost.log")),
        ("permit", unified_dir.join("permit.log")),
        ("report", unified_dir.join("report.log")),
    ];

    let legacy_log_dirs = [
        paths.xdg_data.join("ccu").join("logs"),
        paths.xdg_data.join("claude-permit").join("logs"),
        paths.xdg_data.join("claude-report").join("logs"),
    ]
    .into_iter()
    .filter(|d| d.exists())
    .collect();

    (log_locations, legacy_log_dirs)
}

/// A config file that exists ONLY at its legacy path (not yet migrated to clyde) is unhealthy.
fn check_legacy_only(label: &str, legacy: &Path, clyde: &Path, out: &mut Vec<String>) {
    if legacy.exists() && !clyde.exists() {
        out.push(label.to_string());
    }
}

/// True if the legacy `~/.config/claude-permit/` dir holds a `config.yml` or any `*.yml`.
fn permit_legacy_config_present(paths: &Paths) -> bool {
    let dir = paths.xdg_config.join("claude-permit");
    if dir.join("config.yml").exists() {
        return true;
    }
    std::fs::read_dir(&dir)
        .ok()
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("yml"))
        })
        .unwrap_or(false)
}

fn statusline_target(paths: &Paths) -> Target {
    let path = paths.home.join(".claude").join("statusline.sh");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Target::Absent;
    };
    // Check the legacy form FIRST: a mixed file (both `ccu` and `clyde cost`) means migration is
    // incomplete and must read as legacy, not healthy.
    if text.contains("ccu today") || text.contains("ccu weekly") || text.contains("ccu monthly") {
        Target::Legacy("ccu")
    } else if text.contains("clyde cost") {
        Target::Clyde
    } else {
        Target::Absent
    }
}

fn hook_target(path: &Path) -> Target {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Target::Absent;
    };
    // Legacy form first: a settings file with both commands is incomplete, not healthy.
    if text.contains("claude-permit log") {
        Target::Legacy("claude-permit")
    } else if text.contains("clyde permit log") {
        Target::Clyde
    } else {
        Target::Absent
    }
}

/// Inspect the enrich systemd units by CONTENT, not mere existence. Returns the health target, the
/// active unit name, and its `ExecStart=` line. Legacy iff any `klod-enrich.{service,timer}` file
/// or the `timers.target.wants/klod-enrich.timer` enable symlink remains, OR the active
/// `clyde-enrich.service`'s ExecStart still invokes `klod` (a half-rewritten unit), OR it still uses
/// the pre-rename `sessions enrich` subcommand spelling (a unit that predates `sessions`->`session`).
fn timer_state(paths: &Paths) -> (Target, Option<String>, Option<String>) {
    let dir = paths.xdg_config.join("systemd").join("user");
    let legacy_svc = dir.join("klod-enrich.service");
    let legacy_tmr = dir.join("klod-enrich.timer");
    let legacy_link = dir.join("timers.target.wants").join("klod-enrich.timer");
    let clyde_svc = dir.join("clyde-enrich.service");
    let clyde_tmr = dir.join("clyde-enrich.timer");

    let legacy_present = legacy_svc.exists() || legacy_tmr.exists() || std::fs::symlink_metadata(&legacy_link).is_ok();

    let (unit_name, execstart) = if clyde_svc.exists() {
        (Some("clyde-enrich.service".to_string()), execstart_of(&clyde_svc))
    } else if legacy_svc.exists() {
        (Some("klod-enrich.service".to_string()), execstart_of(&legacy_svc))
    } else {
        (None, None)
    };

    let execstart_legacy = execstart.as_deref().is_some_and(|e| e.contains("klod"));
    // A clyde-named unit whose ExecStart still invokes the pre-rename `sessions enrich` spelling is
    // stale: the timer fires `clyde ... sessions enrich`, which now errors (no `sessions` alias).
    // `clyde bootstrap` rewrites it; flag it here so the broken timer doesn't read as healthy.
    let execstart_stale_subcmd = execstart.as_deref().is_some_and(|e| e.contains("sessions enrich"));
    let target = if legacy_present || execstart_legacy {
        Target::Legacy("klod")
    } else if execstart_stale_subcmd {
        Target::Legacy("sessions enrich")
    } else if clyde_svc.exists() || clyde_tmr.exists() {
        Target::Clyde
    } else {
        Target::Absent
    };
    (target, unit_name, execstart)
}

/// Legacy `klod-*` enrich residue, each entry naming the exact PATH to remove.
///
/// State of the reindex sweep's systemd units: `Clyde` only when the service, the timer, AND the
/// `timers.target.wants` enable symlink are ALL present.
///
/// All three, because any one of them missing is a dead scheduler: a service with no timer never
/// fires, and a timer with no enable symlink is not armed. `symlink_metadata` for the link rather
/// than `exists()`, which follows the link and reports false for a dangling one -- the same reason
/// [`legacy_timer_residue`] uses it.
fn reindex_timer_state(paths: &Paths) -> Target {
    let complete = paths.clyde_reindex_unit().exists()
        && paths.clyde_reindex_timer().exists()
        && std::fs::symlink_metadata(paths.clyde_reindex_wants_link()).is_ok();
    debug!("reindex_timer_state: complete={complete}");
    if complete { Target::Clyde } else { Target::Absent }
}

/// Separate from [`timer_state`], which is left byte-identical to the version that detects correctly
/// today, so retiring the migration needs no new proof that detection still works.
///
/// This exists because [`timer_state`]'s report is illegible for the residue states that matter most.
/// `unit_name` is `Some` only when a `.service` file exists, so on a host whose only residue is
/// `klod-enrich.timer` or a dangling enable symlink, the whole report is one line
/// (`enrich timer: klod (legacy)`): the exit code is right and the operator is never told which file
/// to touch. `legacy_state` prints one line per entry, which is the channel that already works for the
/// dir checks.
fn legacy_timer_residue(paths: &Paths) -> Vec<String> {
    let dir = paths.xdg_config.join("systemd").join("user");
    let candidates = [
        dir.join("klod-enrich.service"),
        dir.join("klod-enrich.timer"),
        dir.join("timers.target.wants").join("klod-enrich.timer"),
    ];
    let residue: Vec<String> = candidates
        .iter()
        // `symlink_metadata`, NOT `exists()`: `exists()` follows the link and returns false for a
        // DANGLING one, which is exactly the residue left by deleting unit files without disabling
        // the timer. See the note on `timer_state`.
        .filter(|p| std::fs::symlink_metadata(p).is_ok())
        .map(|p| format!("legacy klod enrich unit: {}", p.display()))
        .collect();
    debug!("legacy_timer_residue: found={}", residue.len());
    residue
}

/// The trimmed `ExecStart=` line of a unit file, if present.
fn execstart_of(unit: &Path) -> Option<String> {
    let text = std::fs::read_to_string(unit).ok()?;
    text.lines()
        .map(str::trim)
        .find(|l| l.starts_with("ExecStart="))
        .map(str::to_string)
}

fn count_events(db: &Path) -> Result<i64> {
    let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;
    Ok(n)
}

fn print_report(paths: &Paths, report: &Report) {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "(unknown)".to_string());
    println!("{}", "clyde doctor".bold());
    println!("  binary:        {exe}");
    println!("  data:          {}", paths.xdg_data.join("clyde").display());
    println!("  config:        {}", paths.xdg_config.join("clyde").display());
    println!("  cache:         {}", paths.xdg_cache.join("clyde").display());
    println!("  statusline:    {}", report.statusline.label());
    println!("  hook (global): {}", report.hook_global.label());
    println!("  hook (local):  {}", report.hook_local.label());
    println!("  enrich timer:  {}", report.timer.label());
    if let Some(unit) = &report.timer_unit {
        println!("    unit:        {unit}");
    }
    if let Some(exec) = &report.timer_execstart {
        println!("    {exec}");
    }
    println!("  reindex timer: {}", report.reindex_timer.label());
    if report.reindex_timer == Target::Absent {
        // Not a `healthy()` failure: a host can legitimately choose not to schedule the sweep, and
        // clyde must not fail a diagnostic over a policy choice. But it IS the difference between the
        // reap-before-stage race being closed and being open, so it is never silent.
        println!(
            "    {} dormant transcripts are only staged when `clyde session reindex` runs; without \
             this timer a session can be TTL-reaped before it is staged and its spend is then \
             unrecoverable. Install with `clyde bootstrap --install-timer`.",
            "note:".yellow()
        );
    }
    match (report.events_db_at_clyde, report.events_db_rows) {
        (true, Some(n)) => println!("  events DB:     {} ({} rows)", "clyde".green(), n),
        (true, None) => println!("  events DB:     {} (row count unavailable)", "clyde".green()),
        (false, _) if report.events_db_at_legacy => {
            println!("  events DB:     {}", "legacy claude-permit path only".red())
        }
        (false, _) => println!("  events DB:     {}", "absent".dimmed()),
    }
    // When the clyde DB exists the line above reads green, but a legacy events DB alongside it still
    // makes the report unhealthy (`events_db_at_legacy`). Surface it explicitly so the `✗` footer
    // isn't a mystery -- `clyde bootstrap` now merges it in and removes it.
    if report.events_db_at_clyde && report.events_db_at_legacy {
        println!(
            "  {} legacy claude-permit events DB also present (run `clyde bootstrap` to merge)",
            "legacy state:".red()
        );
    }
    for item in &report.legacy_state {
        println!("  {} {}", "legacy state:".red(), item);
    }
    println!("  logs:");
    for (tool, path) in &report.log_locations {
        println!("    {:<8} {}", tool, path.display());
    }
    // Informational only: legacy log dirs are disposable diagnostics, not migration state, so
    // their presence never flips the healthy() verdict (unlike `legacy_state`).
    if !report.legacy_log_dirs.is_empty() {
        println!(
            "  {}",
            "legacy log dirs (informational; safe to leave or archive):".yellow()
        );
        for dir in &report.legacy_log_dirs {
            println!("    {}", dir.display());
        }
    }
    if report.healthy() {
        println!("{}", "✓ all integrations resolve to clyde".green());
    } else if report.has_klod_residue() {
        // `clyde bootstrap` CANNOT fix this any more (design Phase 4 retired the migration), so
        // printing the generic remedy here would be a lie. Naming the real path is the whole point of
        // keeping the detection after deleting the machinery.
        println!(
            "{}",
            "✗ pre-rename `klod` state remains, and this clyde can no longer migrate it. Install a \
             pre-retirement clyde (<= v0.18.0), run `clyde bootstrap`, then upgrade again"
                .red()
        );
    } else {
        println!("{}", "✗ legacy targets/state remain: run `clyde bootstrap`".red());
    }
}

#[cfg(test)]
mod tests;
