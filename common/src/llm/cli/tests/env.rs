#![allow(clippy::unwrap_used)]

//! Split out of the former single 1,322-line `cli/tests.rs` (design Phase 6). The section banners in
//! that file were already the module boundaries; each submodule below is one contiguous run of them.

use super::*;

// ---- AC4: the child inherits NOTHING ----------------------------------------------------------

#[test]
fn child_env_is_an_allowlist_and_leaks_no_secret() {
    // Holds ENV_LOCK even though it mutates nothing: `child_env` READS the environment, and reading
    // the environ block while another test is inside `set_var` is the unsafety window edition 2024
    // made explicit. Every env-touching test takes this lock, readers included.
    let guard = ENV_LOCK.lock().unwrap();
    // Every kind, so the allowlist doctrine is asserted for all of them and not sampled on one.
    let per_kind: Vec<(Kind, Vec<(String, String)>)> = ALL_KINDS.iter().map(|k| (*k, child_env(*k))).collect();
    drop(guard);
    for (kind, env) in &per_kind {
        let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        for name in &names {
            assert!(
                matches!(*name, "HOME" | "PATH" | "NO_UPDATE_NOTIFIER" | MAX_THINKING_TOKENS)
                    || PROXY_VARS.contains(name),
                "unexpected variable in the {kind:?} allowlist: {name}"
            );
        }
        assert!(
            !names.iter().any(|n| n.starts_with("CLAUDE")),
            "no CLAUDE* variable may reach the {kind:?} child: {names:?}"
        );
    }
    let env = per_kind
        .iter()
        .find(|(k, _)| *k == Kind::Slot)
        .map(|(_, e)| e.clone())
        .expect("Kind::Slot is in ALL_KINDS");
    let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();

    // Enumerated BY NAME so a future secret-bearing variable fails loudly rather than leaking.
    for forbidden in [
        "ANTHROPIC_API_KEY",
        "CLAUDE_COST_ANTHROPIC_API_ADMIN_KEY",
        "CLAUDE_COST_SLACK_APP_TOKEN",
        "CLAUDE_COST_SLACK_BOT_TOKEN",
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_CHILD_SESSION",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_EXECPATH",
        "CLAUDE_TMPDIR",
        "CLAUDE_EFFORT",
    ] {
        assert!(
            !names.contains(&forbidden),
            "{forbidden} must not reach the child: {names:?}"
        );
    }
    // And nothing CLAUDE*-shaped at all, so the next such variable is excluded by construction.
    assert!(
        !names.iter().any(|n| n.starts_with("CLAUDE")),
        "no CLAUDE* variable may reach the child: {names:?}"
    );
    // The allowlist is exactly the three documented entries plus the enumerated proxy names (HOME
    // only when resolvable, each proxy name only when set in the parent).
    for name in &names {
        assert!(
            matches!(*name, "HOME" | "PATH" | "NO_UPDATE_NOTIFIER") || PROXY_VARS.contains(name),
            "unexpected variable in the allowlist: {name}"
        );
    }
    assert_eq!(
        env.iter()
            .find(|(k, _)| k == "NO_UPDATE_NOTIFIER")
            .map(|(_, v)| v.as_str()),
        Some("1"),
        "the update-notice guard must be set"
    );
}

/// The child must be told how to reach the network, and must NOT be told a credential.
///
/// BITES: drop the `PROXY_VARS` loop from `child_env` and the four forwarded names go missing (the
/// sandbox failure this fixes); widen it to a `*PROXY*` glob and `CLOUDSDK_PROXY_PASSWORD` appears,
/// which is the secret-leak class the allowlist exists to prevent.
#[test]
fn child_env_forwards_the_proxy_address_and_never_a_proxy_credential() {
    let guard = ENV_LOCK.lock().unwrap();
    let planted = [
        ("HTTP_PROXY", "http://127.0.0.1:8080"),
        ("HTTPS_PROXY", "http://127.0.0.1:8080"),
        ("ALL_PROXY", "socks5://127.0.0.1:1080"),
        ("NO_PROXY", "localhost,127.0.0.1"),
        // NOT a proxy address: a credential that a `*PROXY*` glob would happily forward.
        ("CLOUDSDK_PROXY_PASSWORD", "planted-proxy-password-must-not-leak"),
    ];
    // Capture what was there BEFORE planting, and put it back afterwards -- the sibling test below
    // already does this and it matters more here. `child_env`'s own docs record that this sandboxed
    // CI genuinely depends on real `*_PROXY` vars for the child `claude` process's network egress,
    // so unconditionally removing them left every later test in the binary running without the
    // proxy the runner had set: a permanent wipe, not a cleanup.
    let prior: Vec<(&str, Option<String>)> = planted.iter().map(|(k, _)| (*k, std::env::var(k).ok())).collect();
    // SAFETY: serialized behind ENV_LOCK; every planted var is restored below.
    for (k, v) in planted {
        unsafe { std::env::set_var(k, v) };
    }
    let env = child_env(Kind::Slot);
    unsafe {
        for (name, value) in &prior {
            match value {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
    }
    drop(guard);

    for name in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"] {
        let value = env.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
        assert!(
            value.is_some(),
            "{name} must reach the child or a sandboxed render cannot connect: {env:?}"
        );
    }
    assert!(
        !env.iter().any(|(k, _)| k == "CLOUDSDK_PROXY_PASSWORD"),
        "a proxy CREDENTIAL must never reach the child: {env:?}"
    );
    assert!(
        !env.iter()
            .any(|(_, v)| v.contains("planted-proxy-password-must-not-leak")),
        "the credential's VALUE must not reach the child under any name: {env:?}"
    );
}

/// A proxy variable that is unset (or empty) in the parent adds nothing: the child's environment
/// stays the minimum it can be, and an empty `HTTPS_PROXY=` never masks a real one.
#[test]
fn child_env_forwards_no_proxy_variable_that_is_unset_or_empty() {
    let guard = ENV_LOCK.lock().unwrap();
    let prior: Vec<(&str, Option<String>)> = PROXY_VARS.iter().map(|n| (*n, std::env::var(n).ok())).collect();
    // SAFETY: serialized behind ENV_LOCK; every value is restored below.
    unsafe {
        for name in PROXY_VARS {
            std::env::remove_var(name);
        }
        std::env::set_var("HTTPS_PROXY", "");
    }
    let env = child_env(Kind::Slot);
    unsafe {
        std::env::remove_var("HTTPS_PROXY");
        for (name, value) in &prior {
            match value {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
    }
    drop(guard);

    assert!(
        !env.iter().any(|(k, _)| PROXY_VARS.contains(&k.as_str())),
        "no proxy variable may be invented or forwarded empty: {env:?}"
    );
}

#[test]
fn child_env_survives_a_secret_being_present_in_the_parent() {
    // The parent's env is irrelevant by construction (env_clear + allowlist), so setting a secret
    // here must change nothing. This is the property a denylist could not guarantee.
    let guard = ENV_LOCK.lock().unwrap();
    let before = child_env(Kind::Slot);
    // SAFETY: serialized behind ENV_LOCK; removed before the guard drops.
    unsafe {
        std::env::set_var("CLAUDE_COST_SLACK_BOT_TOKEN", "xoxb-not-a-real-token");
    }
    let after = child_env(Kind::Slot);
    unsafe {
        std::env::remove_var("CLAUDE_COST_SLACK_BOT_TOKEN");
    }
    drop(guard);
    assert_eq!(before, after, "the child env must not depend on the parent's");
}

/// AC4 clause one, proven by inspecting a REAL child's environment.
///
/// This spawns `/usr/bin/env` in place of `claude` and reads what the child actually received. An
/// earlier version of this test asserted `Command::get_envs().len()`, which does NOT work: that
/// getter reports only the explicit OVERRIDES, so deleting `cmd.env_clear()` left the assertion
/// passing while the child silently inherited the parent's entire environment — including the three
/// measured secrets below. The test was green and the security property was gone.
///
/// Nothing about this needs the `claude` binary, so the scope boundary ("no test shells out to the
/// real claude") is respected: `/usr/bin/env` is hermetic, fast, and present everywhere this builds.
///
/// BITES: delete `cmd.env_clear()` in `Spawn::to_command` and this fails on the planted secret.
#[test]
fn built_command_gives_the_child_only_the_allowlist_and_no_inherited_secret() {
    let guard = ENV_LOCK.lock().unwrap();
    // Plant a secret of each shape the design measured as leaking, in the PARENT.
    // Values are long and distinctive on purpose: the value-leak assertion below is a substring
    // search over the child's whole environment, so a short value like "1" would false-positive
    // against a legitimate allowlist entry (`NO_UPDATE_NOTIFIER=1`).
    let planted = [
        ("CLAUDE_COST_ANTHROPIC_API_ADMIN_KEY", "planted-admin-key-must-not-leak"),
        ("CLAUDE_COST_SLACK_BOT_TOKEN", "planted-slack-bot-must-not-leak"),
        ("ANTHROPIC_API_KEY", "planted-api-key-must-not-leak"),
        ("CLAUDECODE", "planted-claudecode-must-not-leak"),
    ];
    // SAFETY: serialized behind ENV_LOCK; every planted var is removed below.
    for (k, v) in planted {
        unsafe { std::env::set_var(k, v) };
    }

    let mut spawn = transport().build_spawn(job(Kind::Slot), "SYS", "P");
    // Swap the program for `env`, which prints the environment it was handed. The env/args split is
    // exactly what a real render builds; only the executable differs.
    spawn.program = PathBuf::from("/usr/bin/env");
    spawn.args.clear();
    let output = spawn.to_command().output().expect("/usr/bin/env must be spawnable");

    for (k, _) in planted {
        unsafe { std::env::remove_var(k) };
    }
    // `child_env` READS the environment (`dirs::home_dir()`, `PATH`), so it must be called while
    // the lock is still held. Reading the environ block concurrently with another test's `set_var` is
    // the same unsafety window that makes `set_var` itself unsafe in edition 2024 — it can tear or
    // crash rather than fail cleanly. The assertion below cannot go WRONG (the allowlist can never
    // contain a planted secret), so this is purely about not reading a block mid-mutation.
    let allowlist = child_env(Kind::Slot);
    drop(guard);

    assert!(output.status.success(), "env exited {:?}", output.status.code());
    let seen: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    let names: Vec<&str> = seen.iter().filter_map(|l| l.split('=').next()).collect();

    // Not one planted secret may appear, by name OR by value.
    for (k, v) in planted {
        assert!(!names.contains(&k), "{k} leaked into the child: {names:?}");
        assert!(
            !seen.iter().any(|l| l.contains(v)),
            "{k}'s VALUE leaked into the child under another name: {names:?}"
        );
    }
    // And the child's whole environment is the allowlist, nothing more. Both sides of this move
    // together if the allowlist changes, which is why the sibling
    // `child_env_is_an_allowlist_and_leaks_no_secret` pins the allowlist to its literal three names —
    // keep the pair together if either is ever refactored.
    let mut got = names.clone();
    got.sort_unstable();
    let mut want: Vec<&str> = allowlist.iter().map(|(k, _)| k.as_str()).collect();
    want.sort_unstable();
    assert_eq!(got, want, "child env must be exactly the allowlist");
}
