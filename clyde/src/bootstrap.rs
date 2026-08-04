//! `clyde bootstrap`: migrate every absorbed tool's config/data/cache under one clyde home and
//! repoint the live integrations (ccu statusline, permit hook, enrich systemd timer) at `clyde`.
//!
//! Idempotent and fail-safe. Order is load-bearing: data and config are migrated FIRST (so a
//! repointed integration finds its state), THEN integration references are rewritten. Disposable
//! caches are not migrated -- they rebuild at the clyde path. Every file is backed up to
//! `<path>.clyde.bak` before it is modified, so a partial run is recoverable and re-runs are
//! no-ops over already-migrated state.

use std::fs;
use std::os::unix::fs as unixfs;
use std::path::{Path, PathBuf};

use clap::Args;
use eyre::{Context, Result};
use log::{debug, info, warn};
use serde_json::Value;

/// Per-connection busy timeout for every events-DB connection opened during the merge. The merge
/// races the live `claude-permit log` / `clyde permit log` hook, so an instant `SQLITE_BUSY` from a
/// concurrent writer must NOT abort the migration; wait up to this long for the lock instead.
/// Mirrors `sessions::db::BUSY_TIMEOUT_MS`.
const EVENTS_BUSY_TIMEOUT_MS: i64 = 5_000;

/// The binary bootstrap resolves off PATH at install/repoint time so it can write the enrich unit's
/// `Environment=PATH=` override (design `2026-07-29-excise-api-key.md` Phase 5, G7). Mirrors
/// `common::llm::cli::CLAUDE_BINARY` and `clyde::resolve_claude`.
const CLAUDE_BINARY: &str = "claude";

/// The environment variable [`resolve_claude_path_env`] reads and composes.
const PATH_ENV: &str = "PATH";

/// Flags for `clyde bootstrap`.
#[derive(Args, Debug, Default)]
pub struct BootstrapArgs {
    /// Re-write config that already exists at the clyde destination (default: leave it).
    /// Integration repointing always applies regardless; this governs only destination config.
    #[arg(long)]
    pub force: bool,

    /// Skip all systemd timer handling (no unit rewrite, no daemon-reload).
    #[arg(long)]
    pub skip_systemd: bool,

    /// Skip the statusline repoint (ccu -> clyde cost). Use when `~/.claude/statusline.sh` is
    /// managed elsewhere (e.g. a dotfiles symlink) and you will repoint it yourself. An existing
    /// ccu-based statusline will break once the old `ccu` binary is gone, so repoint it to
    /// `clyde cost`.
    #[arg(long)]
    pub skip_statusline: bool,

    /// Create the enrich timer unit even if no legacy unit exists (default: repoint existing only).
    #[arg(long)]
    pub install_timer: bool,

    /// Preview the migration WITHOUT performing any side effect: print the ordered list of actions
    /// that WOULD be taken (moves, repoints, daemon-reload) and exit having written nothing -- no
    /// files created/moved/removed, no symlinks, the events DB never opened for writing or
    /// checkpointed, and no `systemctl` shell-outs. Justified despite the "no --dry-run on opt-in
    /// destructive flags" convention because `bootstrap` is DEFAULT-destructive (no opt-in gate),
    /// which is the carve-out: a default-destructive op may offer a preview.
    #[arg(long)]
    pub dry_run: bool,
}

/// The resolved XDG/home roots bootstrap and doctor operate over. Injected so the whole surface
/// is testable against a temp `$HOME` without touching the real machine.
#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    pub xdg_data: PathBuf,
    pub xdg_config: PathBuf,
    pub xdg_cache: PathBuf,
}

impl Paths {
    /// Resolve from the environment, honoring `$HOME`/`$XDG_*_HOME` with the standard fallbacks
    /// (same logic as `session::paths`).
    pub fn from_env() -> Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| eyre::eyre!("could not determine home dir (set HOME)"))?;
        Ok(Self {
            xdg_data: session::paths::xdg_data_dir().unwrap_or_else(|| home.join(".local").join("share")),
            xdg_config: session::paths::xdg_config_dir().unwrap_or_else(|| home.join(".config")),
            xdg_cache: session::paths::xdg_cache_dir().unwrap_or_else(|| home.join(".cache")),
            home,
        })
    }

    fn claude_dir(&self) -> PathBuf {
        self.home.join(".claude")
    }
    fn settings_global(&self) -> PathBuf {
        self.claude_dir().join("settings.json")
    }
    fn settings_local(&self) -> PathBuf {
        self.claude_dir().join("settings.local.json")
    }
    fn statusline(&self) -> PathBuf {
        self.claude_dir().join("statusline.sh")
    }
    fn systemd_dir(&self) -> PathBuf {
        self.xdg_config.join("systemd").join("user")
    }
    /// `pub(crate)` so `doctor` checks the SAME path bootstrap writes, rather than re-composing it.
    pub(crate) fn clyde_unit(&self) -> PathBuf {
        self.systemd_dir().join("clyde-enrich.service")
    }
    fn clyde_timer(&self) -> PathBuf {
        self.systemd_dir().join("clyde-enrich.timer")
    }
    /// The reindex sweep's service unit. Separate from the enrich unit on purpose: enrich makes
    /// off-machine LLM calls and is work-scoped, while reindex is a local index + stage + price pass
    /// with no network dependency, so they get different schedules and must fail independently.
    pub(crate) fn clyde_reindex_unit(&self) -> PathBuf {
        self.systemd_dir().join(CLYDE_REINDEX_SERVICE)
    }
    pub(crate) fn clyde_reindex_timer(&self) -> PathBuf {
        self.systemd_dir().join(CLYDE_REINDEX_TIMER)
    }
    pub(crate) fn clyde_reindex_wants_link(&self) -> PathBuf {
        self.wants_dir().join(CLYDE_REINDEX_TIMER)
    }
    fn wants_dir(&self) -> PathBuf {
        self.systemd_dir().join("timers.target.wants")
    }
    fn clyde_wants_link(&self) -> PathBuf {
        self.wants_dir().join("clyde-enrich.timer")
    }
    pub fn clyde_events_db(&self) -> PathBuf {
        self.xdg_data.join("clyde").join("events.db")
    }
    fn legacy_events_db(&self) -> PathBuf {
        self.xdg_data.join("claude-permit").join("events.db")
    }
}

/// The systemd shell-out seam. The two post-migration `systemctl --user` calls are the only
/// mutation sites OUTSIDE the hermetic `bootstrap()` core; routing them through this port lets a
/// test inject a counting fake and PROVE the outer `run()` gate honors `--dry-run`/`--skip-systemd`
/// (rather than verifying that gate by inspection only). Production uses [`SystemctlCli`], which
/// actually shells out; CI can't run `systemctl`, so it never sees the real impl.
pub trait Systemd {
    /// `systemctl --user daemon-reload`.
    fn daemon_reload(&self);
    /// `systemctl --user start <timer>`.
    fn start_enrich_timer(&self);
    /// `systemctl --user start clyde-reindex.timer`.
    fn start_reindex_timer(&self);
}

/// Production [`Systemd`]: the real best-effort `systemctl --user` shell-outs. Warns on failure;
/// never aborts bootstrap.
pub struct SystemctlCli;

impl Systemd for SystemctlCli {
    fn daemon_reload(&self) {
        daemon_reload();
    }
    fn start_enrich_timer(&self) {
        start_enrich_timer();
    }
    fn start_reindex_timer(&self) {
        start_timer(CLYDE_REINDEX_TIMER);
    }
}

/// Entry point for `clyde bootstrap`. Resolves real paths and runs the migration; the systemd
/// `daemon-reload` (the one step that shells out) is best-effort and lives only here, so the
/// migration core stays hermetic for tests.
pub fn run(args: &BootstrapArgs) -> Result<()> {
    run_with(args, &SystemctlCli)
}

/// `run` with the [`Systemd`] shell-out seam injected, resolving paths from the environment.
pub fn run_with<S: Systemd>(args: &BootstrapArgs, systemd: &S) -> Result<()> {
    let paths = Paths::from_env()?;
    run_paths(&paths, args, systemd)
}

/// The body of `run()` over explicit `paths` and an injected [`Systemd`] seam. Tests drive this
/// against a temp-`$HOME` `Paths` with a counting fake to assert the outer gate
/// (`!dry_run && !skip_systemd && systemd_changed`) is HONORED -- proving a dry-run takes zero
/// systemctl calls and a live run takes them -- rather than verifying that gate by inspection only.
pub fn run_paths<S: Systemd>(paths: &Paths, args: &BootstrapArgs, systemd: &S) -> Result<()> {
    debug!(
        "bootstrap::run: force={} skip_systemd={} skip_statusline={} install_timer={} dry_run={}",
        args.force, args.skip_systemd, args.skip_statusline, args.install_timer, args.dry_run
    );
    let outcome = bootstrap(paths, args)?;
    // The two `systemctl` shell-outs are the only mutation sites OUTSIDE `bootstrap()`, so they are
    // gated here in the outer `run()`. Under dry_run they are NEVER invoked -- the dry-run report
    // names them as planned steps instead. (See the inventory note in the design doc: a gate
    // threaded only into `bootstrap()` would let these two writes escape.)
    if !args.dry_run && !args.skip_systemd && outcome.systemd_changed {
        systemd.daemon_reload();
        // daemon-reload re-reads units but does not start them; the renamed timer is enabled but
        // inactive until next boot otherwise. Arm it now (only if the timer unit actually exists).
        if paths.clyde_timer().exists() {
            systemd.start_enrich_timer();
        }
        // Same for the reindex timer, gated on this run having actually installed it: an enrich-unit
        // repair alone sets `systemd_changed`, and starting a nonexistent timer just logs a failure.
        if outcome.reindex_timer_changed && paths.clyde_reindex_timer().exists() {
            systemd.start_reindex_timer();
        }
    }

    if args.dry_run {
        info!("bootstrap --dry-run: planned steps: {}", outcome.completed.join(", "));
        println!(
            "clyde bootstrap --dry-run: {} step(s) WOULD be performed (nothing was written):",
            outcome.completed.len()
        );
        for step in &outcome.completed {
            println!("  • would: {step}");
        }
        // Mirror the live run's post-step systemd handling as planned (never-invoked) actions.
        if !args.skip_systemd && outcome.systemd_changed {
            println!("  • would: systemctl --user daemon-reload");
            println!("  • would: systemctl --user start {CLYDE_ENRICH_TIMER} (if timer unit present)");
        }
        if outcome.completed.is_empty() && outcome.failed.is_none() {
            println!("  (nothing to migrate: already on clyde or no legacy state found)");
        }
        println!("Dry run: no files were moved, no symlinks created, the events DB was not opened.");
        print_stale_env_file_warning(&outcome);
        if let Some((step, err)) = outcome.failed {
            eprintln!("  ✗ would fail at: {step}");
            return Err(eyre::eyre!("bootstrap --dry-run failed planning step '{step}': {err}"));
        }
        return Ok(());
    }

    info!("bootstrap: completed steps: {}", outcome.completed.join(", "));
    println!("clyde bootstrap: completed {} step(s):", outcome.completed.len());
    for step in &outcome.completed {
        println!("  ✓ {step}");
    }
    if outcome.completed.is_empty() && outcome.failed.is_none() {
        println!("  (nothing to migrate: already on clyde or no legacy state found)");
    }
    println!("Backups (if any) left at <path>.clyde.bak. Run `clyde doctor` to verify.");
    print_stale_env_file_warning(&outcome);
    // A mid-sequence failure reports exactly which steps completed (above), then surfaces the
    // failing step and exits non-zero. Re-running is safe (completed steps are no-ops).
    if let Some((step, err)) = outcome.failed {
        eprintln!("  ✗ failed at: {step}");
        return Err(eyre::eyre!("bootstrap failed at step '{step}': {err}"));
    }
    Ok(())
}

/// Print the operator-visible G6 warning line when [`Outcome::stale_env_file`] is set. Shared by
/// both the dry-run and live branches of [`run_paths`] so the wording cannot drift between them.
fn print_stale_env_file_warning(outcome: &Outcome) {
    if let Some(path) = &outcome.stale_env_file {
        println!(
            "  ! stale credential file found: {}. clyde no longer reads it; remove it (e.g. `rkvr rmrf {}`)",
            path.display(),
            path.display()
        );
    }
}

/// What a bootstrap run did, for reporting and to drive the post-run daemon-reload. On a partial
/// failure, `completed` lists the steps that succeeded and `failed` names the first failing step
/// plus its error string -- so `run()` can report exactly how far it got.
#[derive(Debug, Default)]
pub struct Outcome {
    pub completed: Vec<String>,
    pub systemd_changed: bool,
    /// Whether the reindex service/timer was installed this run. Tracked separately from
    /// `systemd_changed` so `run()` only arms the reindex timer when its unit was actually written:
    /// `systemd_changed` is also set by an enrich-unit repair, and starting a timer whose unit does
    /// not exist is a spurious failure in the operator's log.
    pub reindex_timer_changed: bool,
    pub failed: Option<(String, String)>,
    /// Present when a stale `~/.config/clyde/enrich.env` is still on disk (G6). Nothing reads it any
    /// more, but clyde does not delete it -- see [`check_stale_env_file`]. `run()` surfaces this as an
    /// operator-visible warning line naming the path.
    pub stale_env_file: Option<PathBuf>,
}

/// The hermetic migration core: every step operates on `paths` and never shells out. Steps are
/// ordered data/config first, then integration repointing. Each step is a no-op when its source
/// is absent or its destination is already in place, so the whole thing is idempotent.
pub fn bootstrap(paths: &Paths, args: &BootstrapArgs) -> Result<Outcome> {
    let mut out = Outcome::default();

    // Run a step: record its label on success, no-op on Ok(false), and on the FIRST error record
    // the failing step + error and stop (returning the partial Outcome so the caller can report
    // exactly which steps completed). Backups left by completed steps stay in place.
    macro_rules! step {
        ($label:expr, $body:expr) => {
            match $body {
                Ok(true) => out.completed.push($label.to_string()),
                Ok(false) => {}
                Err(e) => {
                    out.failed = Some(($label.to_string(), format!("{e:?}")));
                    return Ok(out);
                }
            }
        };
    }

    // Every step takes `dry_run`: under it the step computes its no-op/would-act decision exactly
    // as a live run (so the reported plan is faithful) but returns BEFORE performing the
    // fs/DB/symlink mutation. The `step!` macro still records the label for an Ok(true), so the
    // dry-run plan and the live run report the identical step set.
    let dry = args.dry_run;

    // 1. Data/config migration (so a repointed integration finds its state).
    //
    // The pre-rename data/config dir migration is RETIRED (2026-07-30, design Phase 4). Not merely
    // unused: a host still carrying state under the old binary name can no longer be migrated by this
    // binary at all, and must install a pre-retirement `clyde` first. `doctor` keeps every check that
    // DETECTS such a host, names the offending paths, and branches its remedy to say exactly that --
    // so this file carries no reference to the old name, and a grep for it lands in `doctor` alone.
    step!("permit events DB (WAL-safe move)", migrate_events_db(paths, dry));
    step!(
        "permit config -> clyde/permit.yml",
        migrate_permit_config(paths, args.force, dry)
    );
    step!(
        "cost config -> clyde/cost.yml",
        migrate_file(
            &paths.xdg_config.join("ccu").join("ccu.yml"),
            &paths.xdg_config.join("clyde").join("cost.yml"),
            args.force,
            dry,
        )
    );
    step!(
        "pricing overrides merged -> clyde/pricing.json",
        merge_pricing_overrides(paths, args.force, dry)
    );

    // 2. Integration repointing (always applies -- it must be correct).
    // The statusline repoint is skippable: a user-managed statusline (e.g. a dotfiles symlink)
    // is repointed by its owner, and rewriting it here would replace the symlink. It keeps
    // working via the transitional `ccu` shim until then.
    if !args.skip_statusline {
        step!("statusline ccu -> clyde cost", repoint_statusline(paths, dry));
    }
    step!(
        "permit hook (global settings.json)",
        repoint_hook(&paths.settings_global(), dry)
    );
    step!(
        "permit hook (local settings.local.json)",
        repoint_hook(&paths.settings_local(), dry)
    );
    if !args.skip_systemd {
        match ensure_enrich_unit(paths, args.install_timer, dry) {
            Ok(true) => {
                out.completed.push("enrich systemd unit (installed or repaired)".into());
                out.systemd_changed = true;
            }
            Ok(false) => {}
            Err(e) => {
                out.failed = Some(("enrich systemd unit (installed or repaired)".into(), format!("{e:?}")));
                return Ok(out);
            }
        }
        // The reindex sweep's own unit. Installed alongside the enrich unit but tracked separately:
        // it is the scheduled staging pass that closes the reap-before-stage race, and a host with a
        // healthy enrich timer and no reindex timer is still silently losing sessions to the TTL.
        match ensure_reindex_unit(paths, args.install_timer, dry) {
            Ok(true) => {
                out.completed.push("reindex systemd unit + timer (installed)".into());
                out.systemd_changed = true;
                out.reindex_timer_changed = true;
            }
            Ok(false) => {}
            Err(e) => {
                out.failed = Some(("reindex systemd unit + timer (installed)".into(), format!("{e:?}")));
                return Ok(out);
            }
        }
    }

    // 3. Post-migration checks. Read-only, so it runs identically under --dry-run and live: there is
    // nothing to gate.
    out.stale_env_file = check_stale_env_file(paths);

    Ok(out)
}

/// Migrate the permit config: the canonical `claude-permit/config.yml` first, else the
/// single-`*.yml`-in-the-dir fallback. One `Result<bool>` so the step runner can drive it.
fn migrate_permit_config(paths: &Paths, force: bool, dry_run: bool) -> Result<bool> {
    if migrate_file(
        &paths.xdg_config.join("claude-permit").join("config.yml"),
        &paths.xdg_config.join("clyde").join("permit.yml"),
        force,
        dry_run,
    )? {
        return Ok(true);
    }
    migrate_legacy_permit_config(paths, force, dry_run)
}

/// Append `.clyde.bak` to a path's full filename (so `settings.json` -> `settings.json.clyde.bak`).
fn backup_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.clyde.bak", path.display()))
}

/// Back up `path` to `<path>.clyde.bak` before it is modified. Overwrites a prior backup (a
/// re-run's backup reflects the latest pre-write state, which is what recovery wants).
fn backup(path: &Path) -> Result<()> {
    let bak = backup_path(path);
    fs::copy(path, &bak).with_context(|| format!("failed to back up {} to {}", path.display(), bak.display()))?;
    debug!("backup: {} -> {}", path.display(), bak.display());
    Ok(())
}

/// Point `link` at `timer` in the systemd wants directory, replacing whatever is already there.
///
/// Both timer installers ran this same sequence inline. `symlink_metadata`, never `exists()`: the
/// latter FOLLOWS the link and reports false for a dangling one, which would leave a stale link in
/// place and the timer un-enabled. That reasoning was written down at one of the two call sites and
/// not the other, which is the shape of the drift this shared function removes.
fn enable_timer_symlink(paths: &Paths, timer: &Path, link: &Path) -> Result<()> {
    fs::create_dir_all(paths.wants_dir())
        .with_context(|| format!("failed to create {}", paths.wants_dir().display()))?;
    if fs::symlink_metadata(link).is_ok() {
        fs::remove_file(link).with_context(|| format!("failed to replace enable symlink {}", link.display()))?;
    }
    unixfs::symlink(timer, link).with_context(|| format!("failed to create enable symlink {}", link.display()))
}

/// Atomic write that also creates the target's parent directory.
///
/// The write itself is [`common::write_atomic`], which was extracted FROM this function and then
/// never wired back into it. Two copies is how this one fell behind: `common`'s version captures
/// and restores the target's existing mode across the rename, so every call site here now keeps a
/// file's exec bit for free, and `repoint_statusline` no longer needs its own permission dance.
/// The `create_dir_all` stays local because `common` requires the parent to exist and several
/// bootstrap targets (systemd unit dirs) may not yet.
fn write_atomic(target: &Path, contents: &str) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| eyre::eyre!("path has no parent: {}", target.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    common::write_atomic(target, contents.as_bytes())
}

/// Move a single config file `legacy -> dest`. `force` governs overwriting an existing dest.
/// Returns whether a move happened.
fn migrate_file(legacy: &Path, dest: &Path, force: bool, dry_run: bool) -> Result<bool> {
    if !legacy.exists() {
        return Ok(false);
    }
    if dest.exists() && !force {
        debug!(
            "migrate_file: dest {} exists and --force not set; skipping",
            dest.display()
        );
        return Ok(false);
    }
    if dry_run {
        // WOULD move (and back up an existing dest). Report without touching the filesystem.
        return Ok(true);
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    if dest.exists() {
        backup(dest)?;
    }
    fs::rename(legacy, dest).with_context(|| format!("failed to move {} -> {}", legacy.display(), dest.display()))?;
    info!("migrated file {} -> {}", legacy.display(), dest.display());
    Ok(true)
}

/// Fallback for the permit config when the legacy `~/.config/claude-permit/` dir holds a single
/// `*.yml` under a non-`config.yml` name: move the first yml found to `clyde/permit.yml`.
fn migrate_legacy_permit_config(paths: &Paths, force: bool, dry_run: bool) -> Result<bool> {
    let legacy_dir = paths.xdg_config.join("claude-permit");
    let dest = paths.xdg_config.join("clyde").join("permit.yml");
    if !legacy_dir.is_dir() || (dest.exists() && !force) {
        return Ok(false);
    }
    let Some(yml) = fs::read_dir(&legacy_dir).ok().and_then(|rd| {
        rd.filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().and_then(|x| x.to_str()) == Some("yml"))
    }) else {
        return Ok(false);
    };
    migrate_file(&yml, &dest, force, dry_run)
}

/// Open an events-DB connection and apply the merge-wide `busy_timeout`, so a concurrent writer
/// (the live permit-log hook) yields a wait rather than an instant `SQLITE_BUSY` that aborts the
/// migration. Used for EVERY connection opened during the merge (legacy, dest, and the read-only
/// verification reopen).
fn open_events_conn(path: &Path) -> Result<rusqlite::Connection> {
    let conn =
        rusqlite::Connection::open(path).with_context(|| format!("failed to open events DB {}", path.display()))?;
    conn.pragma_update(None, "busy_timeout", EVENTS_BUSY_TIMEOUT_MS)
        .with_context(|| format!("failed to set busy_timeout on events DB {}", path.display()))?;
    Ok(conn)
}

/// Open an events-DB connection READ-ONLY with the merge-wide `busy_timeout`, for the post-merge
/// verification count.
fn open_events_conn_ro(path: &Path) -> Result<rusqlite::Connection> {
    let conn = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("failed to open events DB {} read-only", path.display()))?;
    conn.pragma_update(None, "busy_timeout", EVENTS_BUSY_TIMEOUT_MS)
        .with_context(|| format!("failed to set busy_timeout on events DB {}", path.display()))?;
    Ok(conn)
}

/// Run `PRAGMA wal_checkpoint(TRUNCATE)` and FAIL CLOSED if it could not complete. SQLite reports a
/// lock-blocked checkpoint as `SQLITE_OK` with `busy=1` (the first column of the returned row), NOT
/// as an error -- so a plain `execute_batch` would silently treat a blocked checkpoint as success and
/// the caller would then move/delete the `-wal`, stranding committed frames. Reading the `busy`
/// column and erroring on `busy != 0` lets callers abort BEFORE any rename/delete, leaving the DB
/// intact for a retry.
///
/// The `wal_checkpoint(TRUNCATE)` pragma returns one row of three ints `(busy, log, checkpointed)`;
/// `busy` is column 0. A non-zero `busy` means the truncate could not complete.
fn checkpoint_truncate(conn: &rusqlite::Connection, path: &Path) -> Result<()> {
    let busy: i64 = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE);", [], |r| r.get::<_, i64>(0))
        .with_context(|| format!("failed to checkpoint events DB WAL {}", path.display()))?;
    if busy != 0 {
        return Err(eyre::eyre!(
            "events DB WAL checkpoint blocked (busy) on {}; leaving the DB intact for retry",
            path.display()
        ));
    }
    Ok(())
}

/// WAL-safe move of the permit events DB to the clyde home. Checkpoints the WAL (TRUNCATE) and
/// closes the connection before moving `events.db` plus any `-wal`/`-shm` sidecars together, so
/// no committed rows are stranded in an un-checkpointed WAL. No-op if the legacy DB is absent or
/// the clyde DB already exists.
fn migrate_events_db(paths: &Paths, dry_run: bool) -> Result<bool> {
    let legacy = paths.legacy_events_db();
    let dest = paths.clyde_events_db();
    // A claimed snapshot of the legacy DB mid-merge (see `merge_events_db`): `events.db.merging`.
    let staging = sidecar(&legacy, ".merging");
    debug!(
        "migrate_events_db: {} -> {} dry_run={dry_run}",
        legacy.display(),
        dest.display()
    );
    // Crash recovery: an interrupted merge left a claimed staging file. If the dest also exists,
    // always finish the merge (reusing the staging snapshot) so the claimed rows are not stranded.
    if staging.exists() && dest.exists() {
        if dry_run {
            // WOULD finish the interrupted merge from the staging snapshot. Report from existence
            // alone; do not open any DB (a real run writes to dest).
            return Ok(true);
        }
        return merge_events_db(&legacy, &dest);
    }
    // Pathological: a staging file exists but no dest. The merge claimed the legacy DB but the dest
    // is gone; we cannot reconstruct it here. Warn and leave the staging file for manual recovery
    // (it is a complete copy of the legacy DB) rather than crashing the whole bootstrap.
    if staging.exists() && !dest.exists() {
        warn!(
            "migrate_events_db: staging file {} present without a clyde DB at {}; leaving it for manual recovery",
            staging.display(),
            dest.display()
        );
        return Ok(false);
    }
    if !legacy.exists() {
        return Ok(false);
    }
    if dest.exists() {
        // Both DBs present: the legacy DB holds pre-cutover events the clyde DB never saw. A plain
        // move would clobber the clyde DB; the old no-op stranded the legacy DB forever (and kept
        // `doctor` permanently red, since its remediation -- this very function -- never cleared it).
        // Merge the legacy rows in and remove the legacy DB instead.
        if dry_run {
            // WOULD merge legacy rows into the clyde DB and remove the legacy DB. Report from
            // existence alone; do not open either DB (a real run writes to both).
            return Ok(true);
        }
        return merge_events_db(&legacy, &dest);
    }
    if dry_run {
        // CRITICAL: do NOT open the DB. A real run runs `PRAGMA wal_checkpoint(TRUNCATE)` here -- a
        // WRITE to the user's events DB -- before the gated rename. Dry-run must neither checkpoint
        // nor open the DB in any writing mode; it reports the planned move from existence alone.
        return Ok(true);
    }
    // Checkpoint and close in an inner scope so the connection is dropped before the move. Capture
    // the row count post-checkpoint (best-effort: a degenerate DB may lack the `events` table) so
    // we can verify preservation after the move.
    let pre_count: Option<i64> = {
        let conn = open_events_conn(&legacy)?;
        // FAIL CLOSED: a busy-blocked checkpoint must abort BEFORE the rename below, so the legacy
        // DB (and its `-wal`) are left untouched for a retry rather than moved with stranded frames.
        checkpoint_truncate(&conn, &legacy)?;
        conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
            .ok()
    };
    debug!("migrate_events_db: pre-move row count = {pre_count:?}");
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::rename(&legacy, &dest).with_context(|| format!("failed to move {} -> {}", legacy.display(), dest.display()))?;
    for suffix in ["-wal", "-shm"] {
        let ls = sidecar(&legacy, suffix);
        let ds = sidecar(&dest, suffix);
        if ls.exists() {
            fs::rename(&ls, &ds)
                .with_context(|| format!("failed to move sidecar {} -> {}", ls.display(), ds.display()))?;
        }
    }
    // Defensive: warn (do not abort -- it is already moved) if the row count changed.
    if let Some(pre) = pre_count {
        match open_events_conn_ro(&dest).and_then(|c| {
            c.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
                .context("failed to count rows in moved events DB")
        }) {
            Ok(post) if post != pre => warn!("migrate_events_db: row count changed {pre} -> {post} across the move"),
            Ok(post) => debug!("migrate_events_db: row count preserved ({post})"),
            Err(e) => warn!("migrate_events_db: post-move row-count check failed: {e}"),
        }
    }
    info!("migrated events DB {} -> {}", legacy.display(), dest.display());
    Ok(true)
}

/// Merge the legacy permit events DB into the existing clyde events DB (identical schema). Used when
/// BOTH exist (see [`migrate_events_db`]): a move would clobber the clyde DB and a no-op would
/// strand the legacy rows forever.
///
/// The merge operates on a CLAIMED snapshot to close the concurrent-write window. The live
/// permit-log hook may be appending to `events.db` while we run, so:
///   1. If the staging file (`events.db.merging`) does NOT yet exist, claim the legacy DB
///      atomically: checkpoint its WAL FAIL-CLOSED (a busy-blocked checkpoint aborts before the
///      rename, leaving the legacy DB intact for retry), `rename(legacy -> staging)`, and MOVE the
///      legacy `-wal`/`-shm` alongside the staging file (rather than deleting them) so any straggler
///      frames written by an already-open permit-log fd in the checkpoint→rename window are
///      preserved with the snapshot. A concurrent permit-log invocation that opens after the rename
///      creates a FRESH `events.db` (merged by the NEXT bootstrap) instead of having its writes lost.
///   2. If the staging file ALREADY exists, this is crash recovery from an interrupted merge: reuse
///      it as-is, do NOT re-checkpoint/rename (the legacy DB was already claimed last time).
///
/// Legacy rows are inserted with fresh autoincrement ids (the two DBs have independent `id`
/// sequences, so the `id` column is omitted to avoid PK collisions). The INSERT is content-dedup'd
/// against the clyde DB by a NULL-safe correlated `NOT EXISTS` over all 7 copied columns, so a crash
/// AFTER the INSERT commits but BEFORE the staging file is renamed away cannot double-insert on the
/// next run: a retry merges only the not-yet-present remainder. (Within-staging exact duplicates are
/// PRESERVED -- the subquery only checks the DESTINATION, never the source.)
///
/// On success the staging file is `rename`d to `<legacy>.clyde.bak`, which BOTH leaves a recoverable
/// backup AND removes the staging file in one atomic step; any preserved `-wal`/`-shm` sidecars are
/// moved alongside the `.clyde.bak` so the backup set is a complete, replayable DB. Idempotent: once
/// staging+legacy are gone, the caller's existence guards make a re-run a no-op.
fn merge_events_db(legacy: &Path, dest: &Path) -> Result<bool> {
    debug!("merge_events_db: {} -> {}", legacy.display(), dest.display());
    let staging = sidecar(legacy, ".merging");

    // Step 1: claim the legacy DB into the staging snapshot (skipped on crash-recovery reuse).
    if !staging.exists() {
        // Checkpoint the legacy WAL so every committed row is in the main file BEFORE the rename --
        // the `-wal` is bound to the old filename and would be orphaned by the move otherwise. FAIL
        // CLOSED: a busy-blocked checkpoint must abort BEFORE the rename below, so no staging file is
        // created and the legacy DB + its `-wal` are left intact for a retry.
        {
            let conn = open_events_conn(legacy)?;
            checkpoint_truncate(&conn, legacy)?;
        }
        // Atomically claim: a concurrent permit-log that opens after this creates a fresh events.db.
        fs::rename(legacy, &staging).with_context(|| {
            format!(
                "failed to claim legacy events DB {} -> {}",
                legacy.display(),
                staging.display()
            )
        })?;
        // The checkpoint completed (verified above), but a permit-log fd opened BEFORE the claim could
        // have committed frames into the legacy `-wal` in the tiny window between the checkpoint and
        // the rename. Rather than DELETE the sidecars (which would lose those straggler frames), MOVE
        // them alongside the staging snapshot so they travel with it and are preserved with the
        // `.clyde.bak` at finalize. The sidecars are the ONLY place those frames can still live, so a
        // failed move must FAIL CLOSED: roll the claim all the way back (restore any sidecar already
        // moved, then the main DB) and return an error, leaving the legacy DB whole for a clean retry
        // rather than silently stranding frames.
        let mut moved: Vec<&str> = Vec::new();
        for suffix in ["-wal", "-shm"] {
            let ls = sidecar(legacy, suffix);
            if !ls.exists() {
                continue;
            }
            let ss = sidecar(&staging, suffix);
            if let Err(e) = fs::rename(&ls, &ss) {
                // Best-effort rollback so the pre-claim state (legacy DB + its sidecars) is restored.
                for done in &moved {
                    let _ = fs::rename(sidecar(&staging, done), sidecar(legacy, done));
                }
                let _ = fs::rename(&staging, legacy);
                return Err(eyre::eyre!(
                    "failed to claim legacy events DB sidecar {} -> {} ({e}); rolled back the claim, legacy DB left intact for retry",
                    ls.display(),
                    ss.display()
                ));
            }
            moved.push(suffix);
        }
    } else {
        debug!(
            "merge_events_db: reusing existing staging snapshot {} (crash recovery)",
            staging.display()
        );
    }

    // Step 2: count the staged snapshot. A missing/degenerate `events` table yields None -> nothing
    // to merge, but we still finalize the staging file below.
    let staging_count: Option<i64> = {
        let conn = open_events_conn(&staging)?;
        conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
            .ok()
    };

    // Step 3: merge staged rows into dest, content-dedup'd against dest.
    let dest_before: i64 = {
        let conn = open_events_conn(dest)?;
        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap_or(0);
        if matches!(staging_count, Some(n) if n > 0) {
            // ATTACH the staged DB and copy every row not already present in dest by full content
            // match. Bound parameter, never an interpolated path. `IS` is NULL-safe equality, so a
            // NULL `raw_input`/`risk_tier`/`raw_json` matches a NULL. The NOT EXISTS checks only the
            // DESTINATION `events` (alias `e`), so within-staging exact duplicates are preserved.
            conn.execute("ATTACH DATABASE ?1 AS staging", [staging.to_string_lossy()])
                .context("failed to attach staged events DB")?;
            conn.execute_batch(
                "INSERT INTO events (timestamp, session_id, tool_name, tool_input, raw_input, risk_tier, raw_json)
                 SELECT l.timestamp, l.session_id, l.tool_name, l.tool_input, l.raw_input, l.risk_tier, l.raw_json
                 FROM staging.events AS l
                 WHERE NOT EXISTS (
                   SELECT 1 FROM events e
                   WHERE e.timestamp IS l.timestamp AND e.session_id IS l.session_id
                     AND e.tool_name IS l.tool_name AND e.tool_input IS l.tool_input
                     AND e.raw_input IS l.raw_input AND e.risk_tier IS l.risk_tier AND e.raw_json IS l.raw_json
                 );",
            )
            .context("failed to merge staged events into clyde events DB")?;
            conn.execute_batch("DETACH DATABASE staging;")
                .context("failed to detach staged events DB")?;
        }
        before
    };

    // Step 4: fail-closed verification. A COUNT that ERRORS (a real failure, not a clean zero) must
    // NOT let us discard the staging snapshot -- keep it for a retry and propagate the Err so the
    // legacy data is preserved. A clean count that merely differs from the dedup-aware expectation
    // is only a `warn!` (do not roll back a committed insert).
    if let Some(n) = staging_count {
        let dest_after = open_events_conn_ro(dest)
            .and_then(|c| {
                c.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
                    .context("failed to count rows after merge")
            })
            .context("post-merge verification failed; keeping staging snapshot for retry")?;
        // Dedup means the insert adds AT MOST `n` rows (fewer when some staged rows already match a
        // clyde row), so the expectation is a range, not an equality.
        if dest_after < dest_before || dest_after > dest_before + n {
            warn!(
                "merge_events_db: expected {}..={} rows after merge, found {dest_after}",
                dest_before,
                dest_before + n
            );
        } else {
            debug!("merge_events_db: merged up to {n} staged rows ({dest_before} -> {dest_after}, dedup-aware)");
        }
    }

    // Step 5: finalize. Rename the staging snapshot to `<legacy>.clyde.bak` -- this leaves the
    // recoverable backup AND removes the staging file in one atomic step. (Do NOT use `backup()`:
    // it would name the file `events.db.merging.clyde.bak`.)
    let bak = backup_path(legacy);
    fs::rename(&staging, &bak)
        .with_context(|| format!("failed to finalize merge {} -> {}", staging.display(), bak.display()))?;
    // Move any preserved straggler sidecars alongside the `.clyde.bak` so the backup set is a
    // complete, replayable DB. As with the claim-time move, a failed sidecar move is NOT fatal (the
    // merged rows are already durable) -- `warn!` and continue.
    for suffix in ["-wal", "-shm"] {
        let ss = sidecar(&staging, suffix);
        if ss.exists() {
            let bs = sidecar(&bak, suffix);
            if let Err(e) = fs::rename(&ss, &bs) {
                warn!(
                    "merge_events_db: failed to move sidecar {} -> {} ({e}); continuing",
                    ss.display(),
                    bs.display()
                );
            }
        }
    }
    info!(
        "merged legacy events into {} and finalized the staging snapshot (backup at {})",
        dest.display(),
        bak.display()
    );
    Ok(true)
}

/// `events.db` + `-wal`/`-shm` -> `events.db-wal` etc.
fn sidecar(db: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{}", db.display(), suffix))
}

/// Merge the two disjoint pricing overrides (`ccu/pricing.json`, `cr/pricing.json`) into a single
/// `clyde/pricing.json`. On a key conflict, ccu wins (and the conflict is logged). No-op if dest
/// exists and `--force` is not set, or if neither source exists.
fn merge_pricing_overrides(paths: &Paths, force: bool, dry_run: bool) -> Result<bool> {
    let ccu = paths.xdg_config.join("ccu").join("pricing.json");
    let cr = paths.xdg_config.join("cr").join("pricing.json");
    let dest = paths.xdg_config.join("clyde").join("pricing.json");
    if !ccu.exists() && !cr.exists() {
        return Ok(false);
    }
    if dest.exists() && !force {
        debug!("merge_pricing_overrides: dest exists and --force not set; skipping");
        return Ok(false);
    }
    if dry_run {
        // WOULD merge the sources into clyde/pricing.json. Report without reading/writing -- the
        // would-act decision rests on source/dest existence only, no parse needed.
        return Ok(true);
    }
    let mut merged = serde_json::Map::new();
    // cr first, then ccu (so ccu overrides on conflict).
    for (src, label) in [(&cr, "cr"), (&ccu, "ccu")] {
        if !src.exists() {
            continue;
        }
        let text = fs::read_to_string(src).with_context(|| format!("failed to read {}", src.display()))?;
        let value: Value = serde_json::from_str(&text).with_context(|| format!("failed to parse {}", src.display()))?;
        if let Value::Object(map) = value {
            for (k, v) in map {
                if merged.contains_key(&k) && label == "ccu" {
                    warn!("merge_pricing_overrides: key {k:?} present in both cr and ccu overrides; ccu wins");
                }
                merged.insert(k, v);
            }
        }
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    if dest.exists() {
        backup(&dest)?;
    }
    let body = serde_json::to_string_pretty(&Value::Object(merged)).context("failed to serialize merged pricing")?;
    write_atomic(&dest, &format!("{body}\n"))?;
    info!("merged pricing overrides -> {}", dest.display());
    Ok(true)
}

/// G6: detect (never delete) a stale enrich `.env` file. Phase 5 removed the only code that ever
/// read it (`EnvironmentFile=` in the generated unit) and the only code that ever put it in place
/// (the since-retired pre-rename migration step), so a file at this path is now inert. It may still hold a live
/// credential, and destroying an operator's secret is not a bootstrap's job (`secrets.md`: custody
/// is the operator's channel; see the design's Non-Goals). Read-only, so it is safe to run
/// identically under `--dry-run` and live: there is no mutation to gate.
fn check_stale_env_file(paths: &Paths) -> Option<PathBuf> {
    let path = paths.xdg_config.join("clyde").join("enrich.env");
    debug!("check_stale_env_file: {}", path.display());
    if !path.exists() {
        return None;
    }
    warn!(
        "check_stale_env_file: {} is no longer read by clyde and should be removed",
        path.display()
    );
    Some(path)
}

/// Rewrite the statusline script's `ccu <today|weekly|monthly>` invocations to `clyde cost ...`.
/// No-op if the script is absent or already repointed. Backs up before rewriting.
fn repoint_statusline(paths: &Paths, dry_run: bool) -> Result<bool> {
    let path = paths.statusline();
    if !path.exists() {
        return Ok(false);
    }
    let text = fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let rewritten = rewrite_statusline(&text);
    if rewritten == text {
        return Ok(false);
    }
    if dry_run {
        // A rewrite WOULD happen (the read above is read-only). Report without backing up or writing.
        return Ok(true);
    }
    // The 0755 exec bit Claude Code needs to run the statusline survives the rename because
    // `write_atomic` restores the target's original mode. This used to be hand-rolled here, around
    // a local atomic write that dropped the mode; the shared helper does it for every caller.
    backup(&path)?;
    write_atomic(&path, &rewritten)?;
    info!("repointed statusline {} (ccu -> clyde cost)", path.display());
    Ok(true)
}

/// Pure transform: `ccu today|weekly|monthly` -> `clyde cost today|weekly|monthly`. Only the
/// command-invocation forms are rewritten; comments mentioning `ccu` are left alone.
fn rewrite_statusline(text: &str) -> String {
    let mut out = text.to_string();
    for sub in ["today", "weekly", "monthly", "yesterday", "daily", "session"] {
        out = out.replace(&format!("ccu {sub}"), &format!("clyde cost {sub}"));
    }
    out
}

/// Rewrite the exact `claude-permit log` hook command to `clyde permit log` in a Claude settings
/// file, preserving every other field, matcher, and ordering. No-op if the file is absent or has
/// no legacy hook. Backs up before rewriting.
fn repoint_hook(path: &Path, dry_run: bool) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let text = fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut root: Value = serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))?;
    let changed = rewrite_hook_commands(&mut root);
    if !changed {
        return Ok(false);
    }
    if dry_run {
        // A hook command WOULD be repointed (the read/parse above is read-only; `root` is a local
        // clone, never written back). Report without backing up or writing.
        return Ok(true);
    }
    backup(path)?;
    let body = serde_json::to_string_pretty(&root).context("failed to serialize settings")?;
    write_atomic(path, &format!("{body}\n"))?;
    info!(
        "repointed permit hook in {} (claude-permit log -> clyde permit log)",
        path.display()
    );
    Ok(true)
}

/// Walk `hooks.PreToolUse[].hooks[].command` and replace exactly `claude-permit log` with
/// `clyde permit log`. Returns whether anything changed.
fn rewrite_hook_commands(root: &mut Value) -> bool {
    let mut changed = false;
    let Some(pre) = root
        .get_mut("hooks")
        .and_then(|h| h.get_mut("PreToolUse"))
        .and_then(|p| p.as_array_mut())
    else {
        return false;
    };
    for entry in pre.iter_mut() {
        let Some(hooks) = entry.get_mut("hooks").and_then(|h| h.as_array_mut()) else {
            continue;
        };
        for hook in hooks.iter_mut() {
            if let Some(cmd) = hook.get_mut("command").and_then(|c| c.as_str().map(str::to_string))
                && cmd == "claude-permit log"
            {
                hook["command"] = Value::String("clyde permit log".to_string());
                changed = true;
            }
        }
    }
    changed
}

/// Ensure the enrich systemd user unit is present and current. A dispatch over exactly three cases:
/// repair a drifted `clyde-enrich.service` ([`refresh_clyde_unit`]), install a fresh service + timer +
/// enable symlink ([`install_clyde_timer`], only under `--install-timer`), or do nothing. Returns
/// whether anything changed.
///
/// Renamed from `repoint_systemd` when the pre-rename migration was retired (2026-07-30). It no
/// longer repoints anything from the old binary name, and a name that says otherwise is the
/// says-one-thing-means-another class the house rules forbid. Detection of a host still carrying
/// legacy units lives in `doctor`, which keeps every one of those checks -- this function simply has
/// nothing left to do about them.
fn ensure_enrich_unit(paths: &Paths, install_timer: bool, dry_run: bool) -> Result<bool> {
    debug!("ensure_enrich_unit: install_timer={install_timer} dry_run={dry_run}");
    let clyde_svc = paths.clyde_unit();
    // Checked BEFORE the repair branch, because the repair only ever rewrites the .service. A host
    // with a service but a missing `clyde-enrich.timer` (or a missing `timers.target.wants` link) has
    // a dead scheduler, and the early return below meant `--install-timer` could never reach it: the
    // service exists, so it repaired the service and returned. The sweep then silently never fires.
    // `symlink_metadata` for the link, never `exists()`, which follows it and reports false for a
    // dangling one -- the same reason `doctor::legacy_timer_residue` uses it.
    let timer_incomplete =
        install_timer && (!paths.clyde_timer().exists() || fs::symlink_metadata(paths.clyde_wants_link()).is_err());
    if timer_incomplete {
        if dry_run {
            // WOULD restore the timer + enable symlink. Report without writing.
            return Ok(true);
        }
        // Writes service + timer + link. Rewriting the service is harmless and idempotent: it is
        // `clyde_service_body` either way.
        return install_clyde_timer(paths);
    }
    // An already-installed unit may predate the `sessions`->`session` rename, still carry the retired
    // `EnvironmentFile=` directive, or still refer to a credential clyde no longer reads. Repair it.
    if clyde_svc.exists() {
        return refresh_clyde_unit(&clyde_svc, dry_run);
    }
    if install_timer {
        if dry_run {
            // WOULD install the clyde service + timer + enable symlink. Report without writing.
            return Ok(true);
        }
        return install_clyde_timer(paths);
    }
    Ok(false)
}

/// Repair an already-clyde-named enrich unit by CONVERGING it on [`clyde_service_body`], when any of
/// three triggers fires: the pre-rename `sessions enrich` subcommand spelling, a retired
/// `EnvironmentFile=` directive (Phase 5, G6), or text still referring to a retired credential
/// ([`mentions_retired_credential`]). Reached from [`ensure_enrich_unit`] whenever a
/// `clyde-enrich.service` already exists: a user whose installed unit predates one of those fixes
/// would otherwise be left with a broken, stale-firing, or lying unit. Returns whether a repair
/// happened (or, in dry-run, would happen). No-op if the unit carries none of the three defects.
///
/// **Writes the canonical body rather than line-editing the existing one.** Editing toward a target is
/// what shipped the defect this repairs: Phase 5 of the excision stripped the `EnvironmentFile=`
/// directive by line filtering and left the comment block explaining it, so the unit went on claiming
/// an Anthropic key lived in it. Nothing here parses comments, so no comment-parsing heuristic can be
/// wrong about which ones to keep.
///
/// **`.clyde.bak` cannot be restored wholesale.** [`backup`] copies the PRE-repair unit, credential
/// comment included, so restoring it verbatim re-arms [`mentions_retired_credential`] and the next
/// `clyde bootstrap` discards the customization again. The recovery instruction is: strip the
/// credential comment from the backup, THEN re-apply customizations. On desk.lan the discarded set is
/// two comment lines and nothing else (`Nice=10` was already in the template, and `Documentation=` is
/// adopted into the canonical body), so this is a cost priced for a host we have not met.
fn refresh_clyde_unit(svc: &Path, dry_run: bool) -> Result<bool> {
    debug!("refresh_clyde_unit: svc={} dry_run={}", svc.display(), dry_run);
    let text = fs::read_to_string(svc).with_context(|| format!("failed to read {}", svc.display()))?;
    let has_stale_subcommand = text.contains("sessions enrich");
    let has_environment_file = text
        .lines()
        .any(|line| line.trim_start().starts_with("EnvironmentFile="));
    let has_retired_credential = mentions_retired_credential(&text);
    if !has_stale_subcommand && !has_environment_file && !has_retired_credential {
        return Ok(false);
    }
    if dry_run {
        // WOULD converge the unit on the canonical body. Report without writing.
        return Ok(true);
    }
    backup(svc)?;
    let claude_path_env = resolve_claude_path_env();
    write_atomic(svc, &clyde_service_body(claude_path_env.as_deref()))?;
    info!(
        "converged clyde enrich unit {} on the canonical body (stale_subcommand={has_stale_subcommand} \
         environment_file={has_environment_file} retired_credential={has_retired_credential})",
        svc.display()
    );
    Ok(true)
}

/// Resolve `claude`'s directory off PATH at install/repoint time and compose it with bootstrap's own
/// inherited [`PATH_ENV`], so the enrich unit's `Environment=PATH=` override can find `claude` even
/// when the systemd user manager does not carry an interactive PATH (Phase 5, G7/R7: there is no
/// `import-environment` in dotfiles and no `PATH` in `~/.config/environment.d/`, so today's working
/// resolution is inherited from the login session, not owned by the unit). Returns `None` (having
/// warned) when `claude` cannot be resolved right now, in which case the unit is written with no
/// `Environment=PATH=` override at all -- the pre-fix behavior of relying on whatever PATH the
/// systemd user manager itself carries.
///
/// Writes the SYMLINK's directory (e.g. `~/.local/bin`), never the versioned install target (e.g.
/// `~/.local/share/claude/versions/2.1.220`): `which::which` returns the PATH-search hit itself,
/// never canonicalized through the symlink, which is exactly the stable directory wanted so the
/// unit does not go stale on a `claude` self-update.
fn resolve_claude_path_env() -> Option<String> {
    debug!("resolve_claude_path_env: resolving `{CLAUDE_BINARY}` off PATH");
    let claude = match which::which(CLAUDE_BINARY) {
        Ok(path) => path,
        Err(e) => {
            warn!(
                "resolve_claude_path_env: `{CLAUDE_BINARY}` not found on PATH ({e}); the enrich unit \
                 will carry no explicit PATH override and will rely on the systemd user manager's own \
                 PATH, which may not include it"
            );
            return None;
        }
    };
    let Some(dir) = claude.parent() else {
        warn!(
            "resolve_claude_path_env: resolved `{CLAUDE_BINARY}` path {} has no parent directory",
            claude.display()
        );
        return None;
    };
    let composed = compose_path_env(dir, std::env::var(PATH_ENV).ok().as_deref());
    info!(
        "resolve_claude_path_env: prepending {} to the enrich unit's PATH",
        dir.display()
    );
    Some(composed)
}

/// Pure: prepend `dir` to `inherited` (bootstrap's own `PATH`), or stand alone if `inherited` is
/// absent/empty. Split out from [`resolve_claude_path_env`] so the composition itself is directly
/// testable without a real `which::which` lookup.
fn compose_path_env(dir: &Path, inherited: Option<&str>) -> String {
    match inherited {
        Some(p) if !p.is_empty() => format!("{}:{p}", dir.display()),
        _ => dir.display().to_string(),
    }
}

/// Render the `Environment=PATH=...` unit directive, or an empty string when `claude` could not be
/// resolved (see [`resolve_claude_path_env`]). Single source of truth for the line's exact shape, so
/// [`clyde_service_body`] cannot drift from itself: it is the one caller, and both the fresh-install
/// and repair paths go through it.
fn environment_path_line(claude_path_env: Option<&str>) -> String {
    match claude_path_env {
        Some(composed) => format!("Environment=PATH={composed}\n"),
        None => String::new(),
    }
}

/// The canonical clyde enrich service body. The one body in this codebase: [`install_clyde_timer`]
/// writes it for a fresh install and [`refresh_clyde_unit`] writes it to repair a TRIGGERED drift. Not
/// a reconciler: a unit that drifts in a way no trigger names is left alone. A repair that line-edits
/// an existing unit toward this shape is what stranded a credential comment after Phase 5 of the
/// excision stripped its `EnvironmentFile=` directive; writing one body cannot.
///
/// The `Documentation=` directive and the `# Default sweep:` comment are here because the live
/// desk.lan unit carried both and the previous template did not. Adopting them is what makes the
/// converge lossless for the only host we have actually seen: the one comment that had to survive is
/// now canonical, so no comment-parsing heuristic has to preserve it.
fn clyde_service_body(claude_path_env: Option<&str>) -> String {
    debug!("clyde_service_body: claude_path_env={claude_path_env:?}");
    format!(
        "[Unit]\n\
        Description=clyde session enrichment sweep (work-scoped, dormant)\n\
        Documentation=https://github.com/tatari-tv/clyde\n\
        After=network-online.target\n\
        Wants=network-online.target\n\n\
        [Service]\n\
        Type=oneshot\n\
        {}\
        # Default sweep: dormant (>=7d idle), work-scoped only, incremental.\n\
        ExecStart=%h/.cargo/bin/clyde --log-level info session enrich\n\
        Nice=10\n",
        environment_path_line(claude_path_env)
    )
}

/// The tokens whose presence in a unit's COMMENTS or `EnvironmentFile=` directives means the file
/// still refers to a credential clyde no longer reads. Lowercase; matched case-insensitively.
///
/// `environmentfile` is included so a surviving directive is caught by this check too, not only by
/// `refresh_clyde_unit`'s separate `has_environment_file` trigger -- the two are deliberately
/// redundant, because this one also fires on a COMMENT that merely mentions the directive.
const RETIRED_CREDENTIAL_TOKENS: [&str; 4] = ["environmentfile", "enrich.env", "anthropic", "api key"];

/// True when the unit text still refers to a credential clyde no longer reads. Widens
/// [`refresh_clyde_unit`]'s trigger so a unit whose directive is already gone but whose comment
/// survives is still repaired, and gives `doctor` the same signal.
///
/// Scoped to `#` comment lines and `EnvironmentFile=` directives ON PURPOSE. A blanket
/// `text.contains("anthropic")` would match the `Documentation=` URL of any future anthropic-hosted
/// doc link and rewrite the unit forever. Matches directive and comment TEXT only, never a value: no
/// secret is read, logged, or echoed.
pub(crate) fn mentions_retired_credential(text: &str) -> bool {
    let hit = text.lines().any(|line| {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') && !trimmed.starts_with("EnvironmentFile=") {
            return false;
        }
        let lower = trimmed.to_lowercase();
        RETIRED_CREDENTIAL_TOKENS.iter().any(|token| lower.contains(token))
    });
    debug!("mentions_retired_credential: text bytes={} hit={hit}", text.len());
    hit
}

/// Create a fresh clyde enrich service + timer + enable symlink (only under `--install-timer`
/// when no legacy unit exists). The timer is the scheduler; without it (and its enable symlink)
/// the oneshot service would never fire. Installs no `EnvironmentFile=` (Phase 5, G6: clyde reads no
/// credential file), and resolves `claude` off PATH at install time to write an explicit
/// `Environment=PATH=` override (Phase 5, G7) when possible -- see [`resolve_claude_path_env`].
fn install_clyde_timer(paths: &Paths) -> Result<bool> {
    debug!("install_clyde_timer: paths={paths:?}");
    let svc = paths.clyde_unit();
    let claude_path_env = resolve_claude_path_env();
    let svc_body = clyde_service_body(claude_path_env.as_deref());
    write_atomic(&svc, &svc_body)?;

    let tmr = paths.clyde_timer();
    let tmr_body = "[Unit]\n\
        Description=Daily clyde session enrichment sweep\n\n\
        [Timer]\n\
        OnCalendar=*-*-* 03:00:00\n\
        Persistent=true\n\
        RandomizedDelaySec=300\n\n\
        [Install]\n\
        WantedBy=timers.target\n";
    write_atomic(&tmr, tmr_body)?;

    enable_timer_symlink(paths, &tmr, &paths.clyde_wants_link())?;
    info!("installed clyde enrich service + timer + enable symlink");
    Ok(true)
}

/// The enrich timer unit, as named by [`ensure_enrich_unit`] and the `Paths::clyde_timer` helper.
const CLYDE_ENRICH_TIMER: &str = "clyde-enrich.timer";

/// The reindex sweep's unit names, as installed by [`install_clyde_reindex_timer`] and reported by
/// `doctor`.
pub(crate) const CLYDE_REINDEX_SERVICE: &str = "clyde-reindex.service";
pub(crate) const CLYDE_REINDEX_TIMER: &str = "clyde-reindex.timer";

/// The reindex service body. `ExecStart` is `clyde session reindex`, which (as of the
/// archived-session-spend design) indexes, then STAGES every dormant transcript, then prices the
/// un-annotated rows. Staging on a schedule is what actually closes the reap-before-stage race:
/// without it a transcript can age past Claude Code's TTL before any manual `clyde session stage`
/// runs, and that session's spend is unrecoverable forever.
///
/// No `network-online.target` dependency, unlike the enrich unit: this pass reads local transcripts
/// and writes the local catalog. Nothing here makes an off-machine call, so nothing should wait on
/// the network to fire.
fn clyde_reindex_service_body() -> String {
    "[Unit]\n\
     Description=clyde session reindex sweep (index, stage dormant transcripts, price)\n\
     Documentation=https://github.com/tatari-tv/clyde\n\n\
     [Service]\n\
     Type=oneshot\n\
     # Staging beats Claude Code's transcript TTL: a dormant session gets a durable copy under\n\
     # ~/.local/share/clyde/staged before its live JSONL can age off disk.\n\
     ExecStart=%h/.cargo/bin/clyde --log-level info session reindex\n\
     Nice=10\n"
        .to_string()
}

/// Install the reindex service + timer + enable symlink. Harvested from [`install_clyde_timer`]
/// rather than reimplemented, so both units share the atomic-write and dangling-symlink handling.
///
/// Fires more often than the enrich sweep (every 6h vs daily) because the thing it is racing is a
/// TTL: the more often a durable copy is taken, the smaller the window in which a transcript can be
/// reaped before it is staged. It is cheap to repeat -- `copy_if_newer` compares mtimes, so a sweep
/// with nothing newly dormant is one stat per candidate.
pub(crate) fn install_clyde_reindex_timer(paths: &Paths) -> Result<bool> {
    debug!("install_clyde_reindex_timer: paths={paths:?}");
    write_atomic(&paths.clyde_reindex_unit(), &clyde_reindex_service_body())?;

    let tmr = paths.clyde_reindex_timer();
    let tmr_body = "[Unit]\n\
        Description=Periodic clyde session reindex + transcript staging sweep\n\n\
        [Timer]\n\
        OnCalendar=*-*-* 00/6:15:00\n\
        Persistent=true\n\
        RandomizedDelaySec=300\n\n\
        [Install]\n\
        WantedBy=timers.target\n";
    write_atomic(&tmr, tmr_body)?;

    enable_timer_symlink(paths, &tmr, &paths.clyde_reindex_wants_link())?;
    info!("installed clyde reindex service + timer + enable symlink");
    Ok(true)
}

/// Ensure the reindex service + timer are present, under `--install-timer`. Mirrors
/// [`ensure_enrich_unit`]'s incomplete-timer detection: a host with the service but a missing timer
/// (or a missing/dangling `timers.target.wants` link) has a dead scheduler, and the sweep silently
/// never fires. Returns whether anything changed.
fn ensure_reindex_unit(paths: &Paths, install_timer: bool, dry_run: bool) -> Result<bool> {
    debug!("ensure_reindex_unit: install_timer={install_timer} dry_run={dry_run}");
    if !install_timer {
        return Ok(false);
    }
    let complete = paths.clyde_reindex_unit().exists()
        && paths.clyde_reindex_timer().exists()
        && fs::symlink_metadata(paths.clyde_reindex_wants_link()).is_ok();
    if complete {
        return Ok(false);
    }
    if dry_run {
        // WOULD install the reindex service + timer + enable symlink. Report without writing.
        return Ok(true);
    }
    install_clyde_reindex_timer(paths)
}

/// Best-effort `systemctl --user daemon-reload`. Warns on failure; never aborts bootstrap. Lives
/// outside the hermetic core so tests never shell out.
fn daemon_reload() {
    match std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
    {
        Ok(status) if status.success() => info!("systemctl --user daemon-reload ok"),
        Ok(status) => warn!("systemctl --user daemon-reload exited {status}"),
        Err(e) => warn!("systemctl --user daemon-reload failed to spawn: {e}"),
    }
}

/// Best-effort `systemctl --user start clyde-enrich.timer`. After the unit rename + daemon-reload
/// the (still enabled) timer is not active in the running session -- reload re-reads units, it does
/// not start them -- so the daily enrich would not arm until the next boot. Start it now. Warns on
/// failure; never aborts bootstrap. Lives outside the hermetic core so tests never shell out.
fn start_enrich_timer() {
    start_timer(CLYDE_ENRICH_TIMER);
}

/// Best-effort `systemctl --user start <timer>`, shared by both timers so neither can drift to a
/// different failure policy. Warns on failure; never aborts bootstrap.
fn start_timer(timer: &str) {
    match std::process::Command::new("systemctl")
        .args(["--user", "start", timer])
        .status()
    {
        Ok(status) if status.success() => info!("systemctl --user start {timer} ok"),
        Ok(status) => warn!("systemctl --user start {timer} exited {status}"),
        Err(e) => warn!("systemctl --user start {timer} failed to spawn: {e}"),
    }
}

#[cfg(test)]
mod tests;
