//! `clyde bootstrap`: migrate every absorbed tool's config/data/cache under one clyde home and
//! repoint the live integrations (ccu statusline, permit hook, enrich systemd timer) at `clyde`.
//!
//! Idempotent and fail-safe. Order is load-bearing: data and config are migrated FIRST (so a
//! repointed integration finds its state), THEN integration references are rewritten. Disposable
//! caches are not migrated — they rebuild at the clyde path. Every file is backed up to
//! `<path>.clyde.bak` before it is modified, so a partial run is recoverable and re-runs are
//! no-ops over already-migrated state.

use std::fs;
use std::os::unix::fs as unixfs;
use std::path::{Path, PathBuf};

use clap::Args;
use eyre::{Context, Result};
use log::{debug, info, warn};
use serde_json::Value;

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
    /// managed elsewhere (e.g. a dotfiles symlink): the permanent `ccu` shim keeps an existing
    /// ccu-based statusline working, so leaving it untouched is safe.
    #[arg(long)]
    pub skip_statusline: bool,

    /// Create the enrich timer unit even if no legacy unit exists (default: repoint existing only).
    #[arg(long)]
    pub install_timer: bool,
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
    fn legacy_unit(&self) -> PathBuf {
        self.systemd_dir().join("klod-enrich.service")
    }
    fn clyde_unit(&self) -> PathBuf {
        self.systemd_dir().join("clyde-enrich.service")
    }
    fn legacy_timer(&self) -> PathBuf {
        self.systemd_dir().join("klod-enrich.timer")
    }
    fn clyde_timer(&self) -> PathBuf {
        self.systemd_dir().join("clyde-enrich.timer")
    }
    fn wants_dir(&self) -> PathBuf {
        self.systemd_dir().join("timers.target.wants")
    }
    fn legacy_wants_link(&self) -> PathBuf {
        self.wants_dir().join("klod-enrich.timer")
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

/// Entry point for `clyde bootstrap`. Resolves real paths and runs the migration; the systemd
/// `daemon-reload` (the one step that shells out) is best-effort and lives only here, so the
/// migration core stays hermetic for tests.
pub fn run(args: &BootstrapArgs) -> Result<()> {
    debug!(
        "bootstrap::run: force={} skip_systemd={} skip_statusline={} install_timer={}",
        args.force, args.skip_systemd, args.skip_statusline, args.install_timer
    );
    let paths = Paths::from_env()?;
    let outcome = bootstrap(&paths, args)?;
    if !args.skip_systemd && outcome.systemd_changed {
        daemon_reload();
    }
    info!("bootstrap: completed steps: {}", outcome.completed.join(", "));
    println!("clyde bootstrap: completed {} step(s):", outcome.completed.len());
    for step in &outcome.completed {
        println!("  ✓ {step}");
    }
    if outcome.completed.is_empty() && outcome.failed.is_none() {
        println!("  (nothing to migrate — already on clyde or no legacy state found)");
    }
    println!("Backups (if any) left at <path>.clyde.bak. Run `clyde doctor` to verify.");
    // A mid-sequence failure reports exactly which steps completed (above), then surfaces the
    // failing step and exits non-zero. Re-running is safe (completed steps are no-ops).
    if let Some((step, err)) = outcome.failed {
        eprintln!("  ✗ failed at: {step}");
        return Err(eyre::eyre!("bootstrap failed at step '{step}': {err}"));
    }
    Ok(())
}

/// What a bootstrap run did, for reporting and to drive the post-run daemon-reload. On a partial
/// failure, `completed` lists the steps that succeeded and `failed` names the first failing step
/// plus its error string — so `run()` can report exactly how far it got.
#[derive(Debug, Default)]
pub struct Outcome {
    pub completed: Vec<String>,
    pub systemd_changed: bool,
    pub failed: Option<(String, String)>,
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

    // 1. Data/config migration (so a repointed integration finds its state).
    step!(
        "sessions data dir klod -> clyde",
        migrate_dir(&paths.xdg_data.join("klod"), &paths.xdg_data.join("clyde"))
    );
    step!(
        "config dir klod -> clyde",
        migrate_dir(&paths.xdg_config.join("klod"), &paths.xdg_config.join("clyde"))
    );
    step!("permit events DB (WAL-safe move)", migrate_events_db(paths));
    step!(
        "permit config -> clyde/permit.yml",
        migrate_permit_config(paths, args.force)
    );
    step!(
        "cost config -> clyde/cost.yml",
        migrate_file(
            &paths.xdg_config.join("ccu").join("ccu.yml"),
            &paths.xdg_config.join("clyde").join("cost.yml"),
            args.force,
        )
    );
    step!(
        "pricing overrides merged -> clyde/pricing.json",
        merge_pricing_overrides(paths, args.force)
    );

    // 2. Integration repointing (always applies — it must be correct).
    // The statusline repoint is skippable: a user-managed statusline (e.g. a dotfiles symlink)
    // keeps working via the permanent `ccu` shim, and rewriting it would replace the symlink.
    if !args.skip_statusline {
        step!("statusline ccu -> clyde cost", repoint_statusline(paths));
    }
    step!(
        "permit hook (global settings.json)",
        repoint_hook(&paths.settings_global())
    );
    step!(
        "permit hook (local settings.local.json)",
        repoint_hook(&paths.settings_local())
    );
    if !args.skip_systemd {
        match repoint_systemd(paths, args.install_timer) {
            Ok(true) => {
                out.completed.push("enrich systemd unit klod -> clyde".into());
                out.systemd_changed = true;
            }
            Ok(false) => {}
            Err(e) => {
                out.failed = Some(("enrich systemd unit klod -> clyde".into(), format!("{e:?}")));
                return Ok(out);
            }
        }
    }

    Ok(out)
}

/// Migrate the permit config: the canonical `claude-permit/config.yml` first, else the
/// single-`*.yml`-in-the-dir fallback. One `Result<bool>` so the step runner can drive it.
fn migrate_permit_config(paths: &Paths, force: bool) -> Result<bool> {
    if migrate_file(
        &paths.xdg_config.join("claude-permit").join("config.yml"),
        &paths.xdg_config.join("clyde").join("permit.yml"),
        force,
    )? {
        return Ok(true);
    }
    migrate_legacy_permit_config(paths, force)
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

/// Atomic write: temp file in the target's own dir, then rename over the target.
fn write_atomic(target: &Path, contents: &str) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| eyre::eyre!("path has no parent: {}", target.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let tmp = parent.join(format!(
        ".{}.clyde.tmp",
        target.file_name().and_then(|n| n.to_str()).unwrap_or("clyde")
    ));
    fs::write(&tmp, contents).with_context(|| format!("failed to write temp {}", tmp.display()))?;
    fs::rename(&tmp, target).with_context(|| format!("failed to rename {} -> {}", tmp.display(), target.display()))?;
    Ok(())
}

/// Migrate a directory `legacy -> dest`. If `dest` does not exist, rename the whole dir. If `dest`
/// already exists (e.g. a pre-bootstrap `clyde permit log` created `clyde/events.db`, creating the
/// clyde data dir), MERGE: move each top-level entry from `legacy` into `dest` that does not
/// collide with an existing dest entry (never clobber — leave the legacy copy and warn), then
/// remove `legacy` only if it ends up empty. Returns whether anything moved. This prevents
/// stranding `klod/sessions.db` under the legacy root while runtime reads the clyde path.
fn migrate_dir(legacy: &Path, dest: &Path) -> Result<bool> {
    debug!("migrate_dir: {} -> {}", legacy.display(), dest.display());
    if !legacy.exists() {
        return Ok(false);
    }
    if !dest.exists() {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::rename(legacy, dest)
            .with_context(|| format!("failed to move {} -> {}", legacy.display(), dest.display()))?;
        info!("migrated dir {} -> {}", legacy.display(), dest.display());
        return Ok(true);
    }
    // Destination already populated: merge non-colliding entries.
    fs::create_dir_all(dest).with_context(|| format!("failed to create {}", dest.display()))?;
    let mut moved_any = false;
    let mut collisions = 0usize;
    for entry in fs::read_dir(legacy).with_context(|| format!("failed to read {}", legacy.display()))? {
        let entry = entry.with_context(|| format!("failed to read entry in {}", legacy.display()))?;
        let target = dest.join(entry.file_name());
        if target.exists() {
            warn!(
                "migrate_dir: {} already exists; leaving legacy copy {} in place",
                target.display(),
                entry.path().display()
            );
            collisions += 1;
            continue;
        }
        fs::rename(entry.path(), &target)
            .with_context(|| format!("failed to merge {} -> {}", entry.path().display(), target.display()))?;
        moved_any = true;
    }
    if collisions == 0
        && let Err(e) = fs::remove_dir(legacy)
    {
        warn!(
            "migrate_dir: could not remove emptied legacy dir {}: {e}",
            legacy.display()
        );
    }
    if moved_any {
        info!(
            "merged dir {} -> {} ({} collision(s) left in place)",
            legacy.display(),
            dest.display(),
            collisions
        );
    }
    Ok(moved_any)
}

/// Move a single config file `legacy -> dest`. `force` governs overwriting an existing dest.
/// Returns whether a move happened.
fn migrate_file(legacy: &Path, dest: &Path, force: bool) -> Result<bool> {
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
fn migrate_legacy_permit_config(paths: &Paths, force: bool) -> Result<bool> {
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
    migrate_file(&yml, &dest, force)
}

/// WAL-safe move of the permit events DB to the clyde home. Checkpoints the WAL (TRUNCATE) and
/// closes the connection before moving `events.db` plus any `-wal`/`-shm` sidecars together, so
/// no committed rows are stranded in an un-checkpointed WAL. No-op if the legacy DB is absent or
/// the clyde DB already exists.
fn migrate_events_db(paths: &Paths) -> Result<bool> {
    let legacy = paths.legacy_events_db();
    let dest = paths.clyde_events_db();
    debug!("migrate_events_db: {} -> {}", legacy.display(), dest.display());
    if !legacy.exists() {
        return Ok(false);
    }
    if dest.exists() {
        warn!("migrate_events_db: clyde events DB already exists; leaving legacy in place");
        return Ok(false);
    }
    // Checkpoint and close in an inner scope so the connection is dropped before the move. Capture
    // the row count post-checkpoint (best-effort: a degenerate DB may lack the `events` table) so
    // we can verify preservation after the move.
    let pre_count: Option<i64> = {
        let conn = rusqlite::Connection::open(&legacy)
            .with_context(|| format!("failed to open events DB {}", legacy.display()))?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .context("failed to checkpoint events DB WAL")?;
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
    // Defensive: warn (do not abort — it is already moved) if the row count changed.
    if let Some(pre) = pre_count {
        match rusqlite::Connection::open_with_flags(&dest, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .and_then(|c| c.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0)))
        {
            Ok(post) if post != pre => warn!("migrate_events_db: row count changed {pre} -> {post} across the move"),
            Ok(post) => debug!("migrate_events_db: row count preserved ({post})"),
            Err(e) => warn!("migrate_events_db: post-move row-count check failed: {e}"),
        }
    }
    info!("migrated events DB {} -> {}", legacy.display(), dest.display());
    Ok(true)
}

/// `events.db` + `-wal`/`-shm` -> `events.db-wal` etc.
fn sidecar(db: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{}", db.display(), suffix))
}

/// Merge the two disjoint pricing overrides (`ccu/pricing.json`, `cr/pricing.json`) into a single
/// `clyde/pricing.json`. On a key conflict, ccu wins (and the conflict is logged). No-op if dest
/// exists and `--force` is not set, or if neither source exists.
fn merge_pricing_overrides(paths: &Paths, force: bool) -> Result<bool> {
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

/// Rewrite the statusline script's `ccu <today|weekly|monthly>` invocations to `clyde cost ...`.
/// No-op if the script is absent or already repointed. Backs up before rewriting.
fn repoint_statusline(paths: &Paths) -> Result<bool> {
    let path = paths.statusline();
    if !path.exists() {
        return Ok(false);
    }
    let text = fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let rewritten = rewrite_statusline(&text);
    if rewritten == text {
        return Ok(false);
    }
    // write_atomic renames a fresh temp over the target, which would land 0644 and drop the 0755
    // exec bit Claude Code needs to run the statusline. Capture and re-apply the original mode.
    let perms = fs::metadata(&path).map(|m| m.permissions()).ok();
    backup(&path)?;
    write_atomic(&path, &rewritten)?;
    if let Some(perms) = perms {
        fs::set_permissions(&path, perms).with_context(|| format!("failed to restore perms on {}", path.display()))?;
    }
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
fn repoint_hook(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let text = fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut root: Value = serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))?;
    let changed = rewrite_hook_commands(&mut root);
    if !changed {
        return Ok(false);
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

/// Repoint the enrich systemd user timer from `klod` to `clyde`: rewrite `ExecStart`, move the
/// `EnvironmentFile` to the clyde config dir (permissions preserved, contents never logged), write
/// the unit as `clyde-enrich.service`, and remove the old `klod-enrich.service`. Repoints an
/// existing unit only, unless `install_timer` is set (then it creates the clyde unit from a
/// template). Returns whether the unit changed.
fn repoint_systemd(paths: &Paths, install_timer: bool) -> Result<bool> {
    let legacy_svc = paths.legacy_unit();
    let legacy_tmr = paths.legacy_timer();
    let has_legacy =
        legacy_svc.exists() || legacy_tmr.exists() || fs::symlink_metadata(paths.legacy_wants_link()).is_ok();
    if !has_legacy {
        if install_timer && !paths.clyde_unit().exists() {
            return install_clyde_timer(paths);
        }
        return Ok(false);
    }

    let mut changed = false;

    // The oneshot service: rewrite klod -> clyde, move the API-key env file (perms preserved,
    // contents never logged), back up an existing clyde dest before overwrite, remove the old unit.
    if legacy_svc.exists() {
        let text =
            fs::read_to_string(&legacy_svc).with_context(|| format!("failed to read {}", legacy_svc.display()))?;
        backup(&legacy_svc)?;
        let clyde_svc = paths.clyde_unit();
        if clyde_svc.exists() {
            backup(&clyde_svc)?;
        }
        write_atomic(&clyde_svc, &rewrite_unit(&text))?;
        move_env_file(paths)?;
        if legacy_svc != clyde_svc {
            fs::remove_file(&legacy_svc)
                .with_context(|| format!("failed to remove old unit {}", legacy_svc.display()))?;
        }
        changed = true;
    }

    // The .timer is the actual scheduler (klod-enrich.timer, WantedBy=timers.target, enabled via a
    // symlink in timers.target.wants/). It must be renamed too, and its enable symlink repointed,
    // or the daily enrich sweep silently stops firing after the service is renamed.
    if legacy_tmr.exists() {
        let text =
            fs::read_to_string(&legacy_tmr).with_context(|| format!("failed to read {}", legacy_tmr.display()))?;
        backup(&legacy_tmr)?;
        let clyde_tmr = paths.clyde_timer();
        if clyde_tmr.exists() {
            backup(&clyde_tmr)?;
        }
        write_atomic(&clyde_tmr, &rewrite_unit(&text))?;
        repoint_wants_symlink(paths)?;
        if legacy_tmr != clyde_tmr {
            fs::remove_file(&legacy_tmr)
                .with_context(|| format!("failed to remove old timer {}", legacy_tmr.display()))?;
        }
        changed = true;
    } else {
        // Service present without an adjacent timer file but the enable symlink still points at the
        // legacy timer name: repoint it so the unit set is consistent.
        repoint_wants_symlink(paths)?;
    }

    if changed {
        info!("repointed enrich units klod -> clyde");
    }
    Ok(changed)
}

/// Repoint the `timers.target.wants/klod-enrich.timer` enable symlink to the clyde timer. No-op if
/// the legacy enable link is absent. Creates the clyde link (absolute target, matching the live
/// link style) and removes the legacy one.
fn repoint_wants_symlink(paths: &Paths) -> Result<()> {
    let legacy_link = paths.legacy_wants_link();
    if fs::symlink_metadata(&legacy_link).is_err() {
        return Ok(());
    }
    let clyde_link = paths.clyde_wants_link();
    fs::create_dir_all(paths.wants_dir())
        .with_context(|| format!("failed to create {}", paths.wants_dir().display()))?;
    if fs::symlink_metadata(&clyde_link).is_ok() {
        fs::remove_file(&clyde_link)
            .with_context(|| format!("failed to replace enable symlink {}", clyde_link.display()))?;
    }
    unixfs::symlink(paths.clyde_timer(), &clyde_link)
        .with_context(|| format!("failed to create enable symlink {}", clyde_link.display()))?;
    fs::remove_file(&legacy_link)
        .with_context(|| format!("failed to remove old enable symlink {}", legacy_link.display()))?;
    info!("repointed enrich timer enable symlink -> clyde-enrich.timer");
    Ok(())
}

/// Pure transform of a unit file (service or timer): every `klod` -> `clyde`. The enrich units
/// reference `klod` only in clyde-appropriate places (ExecStart binary, EnvironmentFile path,
/// Description, the `tatari-tv/klod` Documentation URL), so a blanket replace is correct here.
fn rewrite_unit(text: &str) -> String {
    text.replace("klod", "clyde")
}

/// Move `~/.config/klod/enrich.env` -> `~/.config/clyde/enrich.env`, preserving permissions.
fn move_env_file(paths: &Paths) -> Result<()> {
    let legacy = paths.xdg_config.join("klod").join("enrich.env");
    let dest = paths.xdg_config.join("clyde").join("enrich.env");
    if !legacy.exists() || dest.exists() {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let perms = fs::metadata(&legacy).map(|m| m.permissions()).ok();
    fs::rename(&legacy, &dest).with_context(|| format!("failed to move env file {}", legacy.display()))?;
    if let Some(perms) = perms {
        fs::set_permissions(&dest, perms).with_context(|| format!("failed to set perms on {}", dest.display()))?;
    }
    info!("moved enrich env file to clyde config (contents not logged)");
    Ok(())
}

/// Create a fresh clyde enrich service + timer + enable symlink (only under `--install-timer`
/// when no legacy unit exists). The timer is the scheduler; without it (and its enable symlink)
/// the oneshot service would never fire.
fn install_clyde_timer(paths: &Paths) -> Result<bool> {
    let svc = paths.clyde_unit();
    let svc_body = "[Unit]\n\
        Description=clyde session enrichment sweep (work-scoped, dormant)\n\
        After=network-online.target\n\
        Wants=network-online.target\n\n\
        [Service]\n\
        Type=oneshot\n\
        EnvironmentFile=%h/.config/clyde/enrich.env\n\
        ExecStart=%h/.cargo/bin/clyde --log-level info sessions enrich\n\
        Nice=10\n";
    write_atomic(&svc, svc_body)?;

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

    let link = paths.clyde_wants_link();
    fs::create_dir_all(paths.wants_dir())
        .with_context(|| format!("failed to create {}", paths.wants_dir().display()))?;
    if fs::symlink_metadata(&link).is_ok() {
        fs::remove_file(&link).with_context(|| format!("failed to replace enable symlink {}", link.display()))?;
    }
    unixfs::symlink(&tmr, &link).with_context(|| format!("failed to create enable symlink {}", link.display()))?;
    info!("installed clyde enrich service + timer + enable symlink");
    Ok(true)
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

#[cfg(test)]
mod tests;
