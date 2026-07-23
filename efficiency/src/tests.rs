use super::*;

#[test]
fn run_exits_zero_with_default_globals() {
    let args = EfficiencyArgs {};
    let globals = common::Globals::default();
    let code = run(args, globals).expect("run should not error in the phase 1 scaffold");
    assert_eq!(code, 0);
}

#[test]
fn run_exits_zero_with_an_explicit_log_level() {
    // Edge case: an explicit --log-level must not change the (currently trivial) outcome.
    let args = EfficiencyArgs {};
    let globals = common::Globals {
        log_level: Some("debug".to_string()),
    };
    let code = run(args, globals).expect("run should not error with an explicit log level");
    assert_eq!(code, 0);
}
