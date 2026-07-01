//! Crate-root tests for the hook panic-containment boundary (`run` / `contain_log_panics`).
//!
//! Two layers of coverage:
//! - Direct unit tests on [`contain_log_panics`] with an injected byte buffer, so the exact `{}`
//!   marker and returned exit code can be asserted for the Ok, Err, and panic branches.
//! - End-to-end tests that drive the real [`run`] entry point with a test-only panic injected
//!   inside `run_inner`, one panic before setup (proving the boundary wraps the whole log path)
//!   and one inside the dispatch arm.

use super::*;
use std::cell::Cell;
use std::sync::Mutex;

/// Serializes the two env-var-mutating tests below; env mutation is not safe under Rust's default
/// parallel test execution.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Test-only panic injection points inside [`run_inner`]. `Setup` fires at the very top of
/// `run_inner`, BEFORE `setup_logging` (so it never initializes `env_logger`); `Dispatch` fires at
/// the top of the `Command::Log` match arm, before the DB is opened or stdin is read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum InjectPoint {
    Setup,
    Dispatch,
}

thread_local! {
    static INJECT: Cell<Option<InjectPoint>> = const { Cell::new(None) };
}

/// Arm (or disarm with `None`) the panic injection for the current thread. `catch_unwind` runs its
/// closure on the calling thread, so a point armed here is visible inside `run_inner`.
fn arm_inject(point: Option<InjectPoint>) {
    INJECT.with(|c| c.set(point));
}

/// Called from `run_inner` at each checkpoint; panics iff the current thread armed this point.
pub(crate) fn inject_panic(point: InjectPoint) {
    if INJECT.with(Cell::get) == Some(point) {
        panic!("injected test panic at {point:?}");
    }
}

fn log_args() -> PermitArgs {
    PermitArgs { command: Command::Log }
}

#[test]
fn contain_passes_through_ok_exit_code() {
    let mut out = Vec::new();
    let code = contain_log_panics(&mut out, || Ok(3));
    assert_eq!(code, 3);
    assert!(out.is_empty(), "success path must not emit the {{}} marker");
}

#[test]
fn contain_degrades_err_to_empty_json_and_zero() {
    let mut out = Vec::new();
    let code = contain_log_panics(&mut out, || Err(eyre::eyre!("log path failed")));
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8(out).expect("utf8"), "{}\n");
}

#[test]
fn contain_degrades_panic_to_empty_json_and_zero() {
    // A test-only panicking "store path" stand-in: the closure panics the way a future
    // slice/index bug on the log path would. The boundary must still emit `{}` + exit 0.
    let mut out = Vec::new();
    let code = contain_log_panics(&mut out, || -> Result<i32> { panic!("store path exploded") });
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8(out).expect("utf8"), "{}\n");
}

#[test]
fn panic_message_reads_str_and_string_payloads() {
    let str_payload = std::panic::catch_unwind(|| panic!("literal boom")).unwrap_err();
    assert_eq!(panic_message(str_payload.as_ref()), "literal boom");

    let string_payload = std::panic::catch_unwind(|| panic!("formatted {}", 7)).unwrap_err();
    assert_eq!(panic_message(string_payload.as_ref()), "formatted 7");
}

#[test]
fn run_contains_setup_panic_before_dispatch() {
    // Arm a panic at the very top of run_inner, BEFORE setup_logging / any dispatch. The
    // catch_unwind boundary in `run` must contain it and honor the hook contract (exit 0). This
    // proves the boundary wraps the ENTIRE log path, not merely the match arm. No env isolation is
    // needed because the panic fires before `setup_logging` touches the filesystem or env_logger.
    arm_inject(Some(InjectPoint::Setup));
    let result = run(log_args(), common::Globals::default());
    arm_inject(None);
    assert_eq!(result.expect("setup panic must be contained, not propagated"), 0);
}

#[test]
fn run_contains_dispatch_panic() {
    // Arm a panic at the top of the Command::Log arm (after setup + config load, before the DB
    // open / stdin read). Isolate XDG so `setup_logging` writes into a temp dir, never the real
    // home. Whether setup_logging succeeds or its own init were to fail, both run inside the
    // boundary, so the contract still yields exit 0.
    let guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::TempDir::new().expect("temp dir");
    let prior_data = std::env::var("XDG_DATA_HOME").ok();
    let prior_config = std::env::var("XDG_CONFIG_HOME").ok();
    unsafe {
        std::env::set_var("XDG_DATA_HOME", dir.path());
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
    }

    arm_inject(Some(InjectPoint::Dispatch));
    let result = run(log_args(), common::Globals::default());
    arm_inject(None);

    match prior_data {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    match prior_config {
        Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
    }

    assert_eq!(result.expect("dispatch panic must be contained, not propagated"), 0);
    drop(guard);
}

#[test]
fn log_file_path_resolves_under_unified_clyde_logs_dir() {
    // Phase 8 (D3): permit's log moves off the legacy `claude-permit/logs/` dir onto the unified
    // `<xdg-data>/clyde/logs/permit.log` location shared with cost and report.
    let guard = ENV_LOCK.lock().expect("env lock");
    let prior_data = std::env::var("XDG_DATA_HOME").ok();
    let dir = tempfile::TempDir::new().expect("temp dir");
    unsafe { std::env::set_var("XDG_DATA_HOME", dir.path()) };

    let path = crate::log_file_path();
    assert_eq!(path, dir.path().join("clyde").join("logs").join("permit.log"));

    match prior_data {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}
