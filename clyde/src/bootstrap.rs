//! `clyde bootstrap`: migrate every absorbed tool's config/data/cache under one clyde home and
//! repoint the live integrations (ccu statusline, permit hook, enrich systemd timer) at `clyde`.
//!
//! Idempotent and fail-safe. Order is load-bearing: data and config are migrated FIRST (so a
//! repointed integration finds its state), THEN integration references are rewritten. Disposable
//! caches are not migrated — they rebuild at the clyde path. Every file is backed up to
//! `<path>.clyde.bak` before it is modified, so a partial run is recoverable and re-runs are
//! no-ops over already-migrated state.

use std::fs;
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
        "bootstrap::run: force={} skip_systemd={} install_timer={}",
        args.force, args.skip_systemd, args.install_timer
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
    if outcome.completed.is_empty() {
        println!("  (nothing to migrate — already on clyde or no legacy state found)");
    }
    println!("Backups (if any) left at <path>.clyde.bak. Run `clyde doctor` to verify.");
    Ok(())
}

/// What a bootstrap run did, for reporting and to drive the post-run daemon-reload.
#[derive(Debug, Default)]
pub struct Outcome {
    pub completed: Vec<String>,
    pub systemd_changed: bool,
}

/// The hermetic migration core: every step operates on `paths` and never shells out. Steps are
/// ordered data/config first, then integration repointing. Each step is a no-op when its source
/// is absent or its destination is already in place, so the whole thing is idempotent.
pub fn bootstrap(paths: &Paths, args: &BootstrapArgs) -> Result<Outcome> {
    let mut out = Outcome::default();

    // 1. Data/config migration (so a repointed integration finds its state).
    if migrate_dir(&paths.xdg_data.join("klod"), &paths.xdg_data.join("clyde"))? {
        out.completed.push("sessions data dir klod -> clyde".into());
    }
    if migrate_dir(&paths.xdg_config.join("klod"), &paths.xdg_config.join("clyde"))? {
        out.completed.push("config dir klod -> clyde".into());
    }
    if migrate_events_db(paths)? {
        out.completed.push("permit events DB (WAL-safe move)".into());
    }
    let permit_cfg_moved = migrate_file(
        &paths.xdg_config.join("claude-permit").join("config.yml"),
        &paths.xdg_config.join("clyde").join("permit.yml"),
        args.force,
    )? || migrate_legacy_permit_config(paths, args.force)?;
    if permit_cfg_moved {
        out.completed.push("permit config -> clyde/permit.yml".into());
    }
    if migrate_file(
        &paths.xdg_config.join("ccu").join("ccu.yml"),
        &paths.xdg_config.join("clyde").join("cost.yml"),
        args.force,
    )? {
        out.completed.push("cost config -> clyde/cost.yml".into());
    }
    if merge_pricing_overrides(paths, args.force)? {
        out.completed
            .push("pricing overrides merged -> clyde/pricing.json".into());
    }

    // 2. Integration repointing (always applies — it must be correct).
    if repoint_statusline(paths)? {
        out.completed.push("statusline ccu -> clyde cost".into());
    }
    if repoint_hook(&paths.settings_global())? {
        out.completed.push("permit hook (global settings.json)".into());
    }
    if repoint_hook(&paths.settings_local())? {
        out.completed.push("permit hook (local settings.local.json)".into());
    }
    if !args.skip_systemd && repoint_systemd(paths, args.install_timer)? {
        out.completed.push("enrich systemd unit klod -> clyde".into());
        out.systemd_changed = true;
    }

    Ok(out)
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

/// Move a directory `legacy -> dest` if `legacy` exists and `dest` does not. If `dest` already
/// exists we leave both alone (don't clobber a populated clyde dir); returns whether a move
/// happened.
fn migrate_dir(legacy: &Path, dest: &Path) -> Result<bool> {
    debug!("migrate_dir: {} -> {}", legacy.display(), dest.display());
    if !legacy.exists() {
        return Ok(false);
    }
    if dest.exists() {
        warn!(
            "migrate_dir: both {} and {} exist; leaving legacy in place (manual merge)",
            legacy.display(),
            dest.display()
        );
        return Ok(false);
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::rename(legacy, dest).with_context(|| format!("failed to move {} -> {}", legacy.display(), dest.display()))?;
    info!("migrated dir {} -> {}", legacy.display(), dest.display());
    Ok(true)
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
    // Checkpoint and close in an inner scope so the connection is dropped before the move.
    {
        let conn = rusqlite::Connection::open(&legacy)
            .with_context(|| format!("failed to open events DB {}", legacy.display()))?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .context("failed to checkpoint events DB WAL")?;
    }
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
    let legacy = paths.legacy_unit();
    let clyde = paths.clyde_unit();
    if !legacy.exists() {
        if install_timer && !clyde.exists() {
            return install_clyde_timer(paths);
        }
        return Ok(false);
    }
    let text = fs::read_to_string(&legacy).with_context(|| format!("failed to read {}", legacy.display()))?;
    backup(&legacy)?;
    let rewritten = rewrite_unit(&text);
    write_atomic(&clyde, &rewritten)?;
    // Move the env file (API key) preserving permissions; never log its contents.
    move_env_file(paths)?;
    if legacy != clyde {
        fs::remove_file(&legacy).with_context(|| format!("failed to remove old unit {}", legacy.display()))?;
    }
    info!("repointed enrich unit -> {}", clyde.display());
    Ok(true)
}

/// Pure transform of the unit file: `klod` binary/EnvironmentFile -> `clyde`.
fn rewrite_unit(text: &str) -> String {
    text.replace("/.cargo/bin/klod ", "/.cargo/bin/clyde ")
        .replace(".config/klod/enrich.env", ".config/clyde/enrich.env")
        .replace("%h/.config/klod/", "%h/.config/clyde/")
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

/// Create a fresh clyde enrich timer unit (only under `--install-timer` when no legacy exists).
fn install_clyde_timer(paths: &Paths) -> Result<bool> {
    let unit = paths.clyde_unit();
    let body = "[Unit]\n\
        Description=clyde session enrichment\n\n\
        [Service]\n\
        Type=oneshot\n\
        EnvironmentFile=%h/.config/clyde/enrich.env\n\
        ExecStart=%h/.cargo/bin/clyde --log-level info sessions enrich\n";
    write_atomic(&unit, body)?;
    info!("installed clyde enrich unit {}", unit.display());
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
