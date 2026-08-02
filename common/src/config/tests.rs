#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use super::*;
use crate::ENV_LOCK;
use crate::since::DateTz;

#[test]
fn config_default_is_utc() {
    let cfg = Config::default();
    assert_eq!(cfg.date_tz(), DateTz::Utc);
}

#[test]
fn load_from_missing_file_yields_defaults() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    let cfg = load_from(&path).unwrap();
    assert_eq!(cfg, Config::default());
    assert_eq!(cfg.date_tz(), DateTz::Utc);
}

#[test]
fn load_from_empty_file_yields_defaults() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "").unwrap();
    // serde_yaml treats an empty document as null; with all fields defaulted this is still valid.
    let cfg = load_from(&path).unwrap();
    assert_eq!(cfg.date_tz(), DateTz::Utc);
}

#[test]
fn load_from_local() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "date-tz: local\n").unwrap();
    let cfg = load_from(&path).unwrap();
    assert_eq!(cfg.date_tz(), DateTz::Local);
}

#[test]
fn load_from_utc_explicit() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "date-tz: utc\n").unwrap();
    let cfg = load_from(&path).unwrap();
    assert_eq!(cfg.date_tz(), DateTz::Utc);
}

#[test]
fn render_format_defaults_to_markdown() {
    assert_eq!(Config::default().render_format(), FormatConfig::Markdown);
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "date-tz: utc\n").unwrap();
    assert_eq!(load_from(&path).unwrap().render_format(), FormatConfig::Markdown);
}

#[test]
fn load_from_reads_render_format() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "render:\n  format: marquee-markdown\n").unwrap();
    assert_eq!(load_from(&path).unwrap().render_format(), FormatConfig::MarqueeMarkdown);
}

/// The retired html formats must be REJECTED by name rather than tolerated. `deny_unknown_fields`
/// does not cover an enum VALUE, so this is the assertion that a stale `format: html` fails loudly
/// instead of silently resolving to markdown.
#[test]
fn load_from_rejects_the_retired_html_formats() {
    for value in ["html", "marquee-html"] {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("clyde.yml");
        std::fs::write(&path, format!("render:\n  format: {value}\n")).unwrap();
        let err = load_from(&path).unwrap_err();
        assert!(
            format!("{err:#}").contains(value),
            "a retired format must be named in the error: {err:#}"
        );
    }
}

#[test]
fn load_from_rejects_unknown_render_field() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "render:\n  bogus: 1\n").unwrap();
    assert!(
        load_from(&path).is_err(),
        "deny_unknown_fields should reject `render.bogus`"
    );
}

#[test]
fn load_from_rejects_bad_render_format() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "render:\n  format: docx\n").unwrap();
    assert!(load_from(&path).is_err(), "unknown format variant should fail to parse");
}

// ---- the render output ceilings -----------------------------------------------------------------

#[test]
fn render_ceilings_default_when_the_file_is_absent() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    let cfg = load_from(&path).unwrap();
    assert_eq!(cfg.render_judge_max_output_tokens(), DEFAULT_JUDGE_MAX_OUTPUT_TOKENS);
    assert_eq!(cfg.render_slot_max_output_tokens(), DEFAULT_SLOT_MAX_OUTPUT_TOKENS);
}

#[test]
fn render_ceilings_come_from_clyde_yml_when_set() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    // Sentinels neither ceiling could ever default to, and different from each other, so a crossed
    // pair or a hardcoded value fails here.
    std::fs::write(
        &path,
        "render:\n  judge-max-output-tokens: 12345\n  slot-max-output-tokens: 543\n",
    )
    .unwrap();
    let cfg = load_from(&path).unwrap();
    assert_eq!(cfg.render_judge_max_output_tokens(), 12_345);
    assert_eq!(cfg.render_slot_max_output_tokens(), 543);
}

#[test]
fn render_ceilings_are_independent_of_each_other() {
    // Setting only one must leave the other at its default, not zero it.
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "render:\n  slot-max-output-tokens: 543\n").unwrap();
    let cfg = load_from(&path).unwrap();
    assert_eq!(
        cfg.render_judge_max_output_tokens(),
        DEFAULT_JUDGE_MAX_OUTPUT_TOKENS,
        "an unset ceiling keeps its default"
    );
    assert_eq!(cfg.render_slot_max_output_tokens(), 543);
}

/// AC-C2: a ceiling of `0` is never a legitimate budget, and the error must name the KEY.
///
/// The hand-written `RenderConfig::default` only protects the ABSENT case, so this is the explicit-zero
/// half. Naming the key matters because serde_yaml renders a `de::Error::custom` with the enclosing
/// SECTION and the source location but never the field -- `render: <msg> at line 2 column 3` -- which is
/// why there are two validators rather than one shared one.
///
/// BITES: drop `deserialize_with` from the field and this fails (a 0 would load clean).
#[test]
fn render_rejects_a_zero_judge_ceiling_by_name() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "render:\n  judge-max-output-tokens: 0\n").unwrap();
    let err = format!("{:#}", load_from(&path).unwrap_err());
    assert!(
        err.contains("render.judge-max-output-tokens"),
        "must name the key the user set: {err}"
    );
}

/// The keys retired with `Kind::Markdown` must FAIL, not be silently ignored.
///
/// `render.markdown-model` -> `render.model` and `render.markdown-max-output-tokens` ->
/// `render.judge-max-output-tokens` (design "Render Inversion"). Without `deny_unknown_fields` on
/// `RenderConfig` a stale `clyde.yml` would load clean and silently run the DEFAULT model, which is
/// the worst outcome: the user set a pin, the pin did nothing, and nothing said so.
///
/// BITES: drop `#[serde(deny_unknown_fields)]` from `RenderConfig` and both halves fail.
#[test]
fn render_rejects_the_retired_markdown_keys_by_name() {
    for retired in ["markdown-model: claude-opus-4-8", "markdown-max-output-tokens: 32000"] {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("clyde.yml");
        std::fs::write(&path, format!("render:\n  {retired}\n")).unwrap();
        let err = format!("{:#}", load_from(&path).unwrap_err());
        let key = retired.split(':').next().unwrap();
        assert!(
            err.contains(key),
            "a retired key must fail loudly and name itself, got: {err}"
        );
    }
}

/// The slot half of AC-C2. Separate because each key is named by its own validator, and a single
/// shared one naming neither would pass a test that only checked one side.
#[test]
fn render_rejects_a_zero_slot_ceiling_by_name() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "render:\n  slot-max-output-tokens: 0\n").unwrap();
    let err = format!("{:#}", load_from(&path).unwrap_err());
    assert!(
        err.contains("render.slot-max-output-tokens"),
        "must name the key the user set: {err}"
    );
}

#[test]
fn load_from_rejects_unknown_field() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "date-tz: utc\nbogus: 1\n").unwrap();
    assert!(load_from(&path).is_err(), "deny_unknown_fields should reject `bogus`");
}

#[test]
fn load_from_rejects_bad_enum() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "date-tz: pacific\n").unwrap();
    assert!(load_from(&path).is_err(), "unknown enum variant should fail to parse");
}

#[test]
fn mcp_serve_config_defaults_when_absent() {
    // A from-scratch default and a missing file must agree: reindex-on-start ON, projects-dir the
    // platform `~/.claude/projects`. (Guards the hand-written `impl Default` against the derived
    // `bool` zero-value footgun.)
    let cfg = Config::default();
    assert!(cfg.reindex_on_start(), "reindex-on-start must default to true");
    assert!(
        cfg.projects_dir().ends_with(".claude/projects"),
        "projects-dir default must be ~/.claude/projects, got {}",
        cfg.projects_dir().display()
    );

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    let loaded = load_from(&path).unwrap();
    assert_eq!(loaded, Config::default(), "a missing file must equal Config::default()");
    assert!(loaded.reindex_on_start());
    assert!(loaded.projects_dir().ends_with(".claude/projects"));
}

#[test]
fn mcp_serve_config_override_from_clyde_yml() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "projects-dir: /tmp/custom-projects\nreindex-on-start: false\n").unwrap();
    let cfg = load_from(&path).unwrap();
    assert_eq!(cfg.projects_dir(), PathBuf::from("/tmp/custom-projects"));
    assert!(!cfg.reindex_on_start(), "reindex-on-start override to false must stick");
}

#[test]
fn mcp_serve_config_partial_override_keeps_other_default() {
    // Only reindex-on-start set: projects-dir must still resolve to the platform default.
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "reindex-on-start: false\n").unwrap();
    let cfg = load_from(&path).unwrap();
    assert!(!cfg.reindex_on_start());
    assert!(cfg.projects_dir().ends_with(".claude/projects"));
}

#[test]
fn load_from_rejects_malformed_reindex_on_start() {
    // A non-bool value must fail loud rather than silently defaulting.
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "reindex-on-start: maybe\n").unwrap();
    assert!(
        load_from(&path).is_err(),
        "a non-bool reindex-on-start must fail to parse"
    );
}

#[test]
fn efficiency_defaults_when_absent() {
    // A from-scratch default and a missing file must agree on every efficiency threshold. (Guards
    // the hand-written `impl Default for EfficiencyConfig` against the derived-zero-value footgun:
    // a derived Default would give floor 0.0 / ceiling 0.0 / gates 0, all wrong.)
    let eff = Config::default();
    let eff = eff.efficiency();
    assert_eq!(eff.cache_read_share_floor(), 0.6);
    assert_eq!(eff.tool_error_rate_ceiling(), 0.05);
    assert!(eff.auto_compaction_flag());
    assert_eq!(eff.minimum_total_tokens(), 20000);
    assert_eq!(eff.minimum_turns(), 3);

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    let loaded = load_from(&path).unwrap();
    assert_eq!(loaded, Config::default(), "a missing file must equal Config::default()");
    assert_eq!(loaded.efficiency().cache_read_share_floor(), 0.6);
}

#[test]
fn efficiency_override_from_clyde_yml() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(
        &path,
        "efficiency:\n  cache-read-share-floor: 0.4\n  tool-error-rate-ceiling: 0.1\n  \
         auto-compaction-flag: false\n  minimum-total-tokens: 5000\n  minimum-turns: 2\n",
    )
    .unwrap();
    let cfg = load_from(&path).unwrap();
    let eff = cfg.efficiency();
    assert_eq!(eff.cache_read_share_floor(), 0.4);
    assert_eq!(eff.tool_error_rate_ceiling(), 0.1);
    assert!(!eff.auto_compaction_flag());
    assert_eq!(eff.minimum_total_tokens(), 5000);
    assert_eq!(eff.minimum_turns(), 2);
}

#[test]
fn efficiency_partial_override_keeps_other_defaults() {
    // Only one field set: the rest must still resolve to their hand-written defaults.
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "efficiency:\n  cache-read-share-floor: 0.75\n").unwrap();
    let cfg = load_from(&path).unwrap();
    let eff = cfg.efficiency();
    assert_eq!(eff.cache_read_share_floor(), 0.75);
    assert_eq!(eff.tool_error_rate_ceiling(), 0.05, "unset field keeps its default");
    assert!(eff.auto_compaction_flag());
    assert_eq!(eff.minimum_total_tokens(), 20000);
}

#[test]
fn efficiency_rejects_unknown_field() {
    // deny_unknown_fields: a typo'd efficiency key must fail LOUD, never silently widen behavior.
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "efficiency:\n  cache-read-share-flooor: 0.4\n").unwrap();
    assert!(
        load_from(&path).is_err(),
        "deny_unknown_fields should reject the typo'd `cache-read-share-flooor`"
    );
}

#[test]
fn efficiency_rejects_bad_type() {
    // A non-numeric threshold must fail to parse rather than silently defaulting.
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "efficiency:\n  minimum-turns: three\n").unwrap();
    assert!(
        load_from(&path).is_err(),
        "a non-integer minimum-turns must fail to parse"
    );
}

#[test]
fn efficiency_rejects_cache_read_share_floor_above_one() {
    // A fraction threshold above 1.0 would flag nothing; reject it at parse time (fail closed).
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "efficiency:\n  cache-read-share-floor: 1.1\n").unwrap();
    assert!(
        load_from(&path).is_err(),
        "a cache-read-share-floor above 1.0 must be rejected"
    );
}

#[test]
fn efficiency_rejects_negative_tool_error_ceiling() {
    // A negative fraction would flag everything; reject it.
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "efficiency:\n  tool-error-rate-ceiling: -0.1\n").unwrap();
    assert!(
        load_from(&path).is_err(),
        "a negative tool-error-rate-ceiling must be rejected"
    );
}

#[test]
fn efficiency_rejects_non_finite_threshold() {
    // A non-finite (.nan/.inf) threshold would defeat every comparison; reject it.
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "efficiency:\n  cache-read-share-floor: .nan\n").unwrap();
    assert!(
        load_from(&path).is_err(),
        "a non-finite cache-read-share-floor must be rejected"
    );
}

#[test]
fn efficiency_accepts_valid_boundary_fractions() {
    // The valid range is inclusive of both ends: 0.0 and 1.0 must parse.
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(
        &path,
        "efficiency:\n  cache-read-share-floor: 0.0\n  tool-error-rate-ceiling: 1.0\n",
    )
    .unwrap();
    let cfg = load_from(&path).unwrap();
    assert_eq!(cfg.efficiency().cache_read_share_floor(), 0.0);
    assert_eq!(cfg.efficiency().tool_error_rate_ceiling(), 1.0);
}

// ---- repo-roots ----------------------------------------------------------------------------------

#[test]
fn repo_roots_defaults_to_one_home_repos() {
    assert_eq!(
        Config::default().repo_roots().len(),
        1,
        "the default is exactly one root, not zero and not two"
    );
    assert!(
        Config::default().repo_roots()[0].ends_with("repos"),
        "repo-roots default must be [<home>/repos], got {:?}",
        Config::default().repo_roots()
    );

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    let cfg = load_from(&path).unwrap();
    assert_eq!(cfg, Config::default(), "a missing file must equal Config::default()");
    assert!(cfg.repo_roots()[0].ends_with("repos"));
}

#[test]
fn repo_roots_override_from_clyde_yml() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("clones");
    std::fs::create_dir_all(&root).unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, format!("repo-roots: [{}]\n", root.display())).unwrap();
    // The TempDir path may itself be a symlink (`/tmp` -> `/private/tmp`), so the loaded list holds
    // the configured spelling and possibly its canonical twin. Both must be present; neither may be
    // dropped.
    let loaded = load_from(&path).unwrap();
    assert!(
        loaded.repo_roots().contains(&root),
        "the configured spelling must survive: {:?}",
        loaded.repo_roots()
    );
    assert!(
        loaded.repo_roots().contains(&root.canonicalize().unwrap()),
        "the canonical spelling must be present too: {:?}",
        loaded.repo_roots()
    );
}

/// P1: two roots both load. One `PathBuf` silently cost the second root all attribution, which is
/// Stephen's measured layout.
///
/// BITES: make the field a `PathBuf` again and this cannot even parse.
#[test]
fn repo_roots_accepts_more_than_one() {
    let dir = tempfile::TempDir::new().unwrap();
    let a = dir.path().join("code/work");
    let b = dir.path().join("wt");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(
        &path,
        format!("repo-roots:\n  - {}\n  - {}\n", a.display(), b.display()),
    )
    .unwrap();
    let loaded = load_from(&path).unwrap();
    assert!(loaded.repo_roots().contains(&a), "{:?}", loaded.repo_roots());
    assert!(loaded.repo_roots().contains(&b), "{:?}", loaded.repo_roots());
}

/// A relative root can never match an absolute cwd, so rule 4 would silently stop firing. Reject it
/// at load, naming the key.
///
/// BITES: drop `deserialize_with` from the field and this loads clean.
#[test]
fn repo_roots_rejects_a_relative_path_by_name() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "repo-roots: [repos]\n").unwrap();
    let err = format!("{:#}", load_from(&path).unwrap_err());
    assert!(err.contains("repo-roots"), "must name the key the user set: {err}");
    assert!(err.contains("absolute"), "must say what is wrong: {err}");
}

/// A typo'd root matches nothing and degrades attribution with no signal, so a nonexistent
/// directory is a load error rather than a silent no-op.
#[test]
fn repo_roots_rejects_a_missing_directory() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    let missing = dir.path().join("no-such-root");
    std::fs::write(&path, format!("repo-roots: [{}]\n", missing.display())).unwrap();
    let err = format!("{:#}", load_from(&path).unwrap_err());
    assert!(err.contains("repo-roots"), "must name the key the user set: {err}");
    assert!(err.contains("existing directories"), "must say what is wrong: {err}");
}

/// A file is not a directory: the check is `is_dir`, not `exists`.
#[test]
fn repo_roots_rejects_a_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, "").unwrap();
    std::fs::write(&path, format!("repo-roots: [{}]\n", file.display())).unwrap();
    assert!(load_from(&path).is_err(), "a file must not pass as a repo root");
}

/// `repo-roots: []` is indistinguishable in effect from "no attribution at all". Saying so at load
/// beats discovering it from a report full of unattributed sessions.
///
/// BITES: delete the emptiness check and this loads a config that silently attributes nothing.
#[test]
fn repo_roots_rejects_an_empty_list() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "repo-roots: []\n").unwrap();
    let err = format!("{:#}", load_from(&path).unwrap_err());
    assert!(err.contains("repo-roots"), "must name the key: {err}");
    assert!(err.contains("at least one"), "must say what is wrong: {err}");
}

/// A nested pair has no correct answer: with `<dir>` and `<dir>/tatari-tv` both configured, the cwd
/// `<dir>/tatari-tv/clyde/src` yields the slug `clyde/src` under the longer root. Refuse the pair
/// rather than invent a tie-break, and name BOTH so the operator knows which to remove.
///
/// BITES: delete the overlap loop and this loads, and `slug_under_roots` starts returning
/// `clyde/src`.
#[test]
fn repo_roots_rejects_a_nested_pair_naming_both() {
    let dir = tempfile::TempDir::new().unwrap();
    let outer = dir.path().join("repos");
    let inner = outer.join("tatari-tv");
    std::fs::create_dir_all(&inner).unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(
        &path,
        format!("repo-roots:\n  - {}\n  - {}\n", outer.display(), inner.display()),
    )
    .unwrap();
    let err = format!("{:#}", load_from(&path).unwrap_err());
    assert!(err.contains("repo-roots"), "must name the key: {err}");
    assert!(err.contains("nest"), "must say what is wrong: {err}");
    assert!(err.contains("tatari-tv"), "must name the inner root: {err}");
}

/// Overlap is checked AFTER canonicalization. Two roots that do not overlap textually DO overlap
/// when one is a symlink into the other, and matching both spellings would resurrect the exact
/// ambiguity the rejection exists to prevent.
///
/// BITES: compare the configured strings instead of the canonical paths and this loads clean.
#[test]
#[cfg(unix)]
fn repo_roots_rejects_an_overlap_that_only_exists_after_canonicalization() {
    let dir = tempfile::TempDir::new().unwrap();
    let real = dir.path().join("repos");
    std::fs::create_dir_all(real.join("tatari-tv")).unwrap();
    let link = dir.path().join("code");
    std::os::unix::fs::symlink(real.join("tatari-tv"), &link).unwrap();

    let path = dir.path().join("clyde.yml");
    std::fs::write(
        &path,
        format!("repo-roots:\n  - {}\n  - {}\n", real.display(), link.display()),
    )
    .unwrap();
    let err = format!("{:#}", load_from(&path).unwrap_err());
    assert!(err.contains("nest"), "the canonical pair overlaps: {err}");
}

/// The same root spelled two ways is not two roots. Caught by the same canonical comparison, with a
/// message that says "distinct" rather than "nest", because "X is inside X" reads as a bug.
#[test]
#[cfg(unix)]
fn repo_roots_rejects_two_spellings_of_one_root() {
    let dir = tempfile::TempDir::new().unwrap();
    let real = dir.path().join("repos");
    std::fs::create_dir_all(&real).unwrap();
    let link = dir.path().join("clones");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let path = dir.path().join("clyde.yml");
    std::fs::write(
        &path,
        format!("repo-roots:\n  - {}\n  - {}\n", real.display(), link.display()),
    )
    .unwrap();
    let err = format!("{:#}", load_from(&path).unwrap_err());
    assert!(err.contains("distinct"), "must say the two are one root: {err}");
}

/// A root reached through a symlink is stored in BOTH spellings, because the matchers are lexical
/// and rule 4 runs when the cwd is GONE and cannot be canonicalized retroactively.
///
/// BITES: store only the configured spelling and the canonical assertion fails; store only the
/// canonical one and the configured assertion fails. Either way rule 4 silently stops firing for
/// half the cwds, which is matrix row 24's defect in rule 4's clothing.
#[test]
#[cfg(unix)]
fn repo_roots_keeps_both_spellings_of_a_symlinked_root() {
    let dir = tempfile::TempDir::new().unwrap();
    let real = dir.path().join("real-repos");
    std::fs::create_dir_all(&real).unwrap();
    let link = dir.path().join("repos");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, format!("repo-roots: [{}]\n", link.display())).unwrap();
    let loaded = load_from(&path).unwrap();
    assert!(
        loaded.repo_roots().contains(&link),
        "the configured spelling must match a cwd recorded through the link: {:?}",
        loaded.repo_roots()
    );
    assert!(
        loaded.repo_roots().contains(&real.canonicalize().unwrap()),
        "the canonical spelling must match a cwd recorded through the real path: {:?}",
        loaded.repo_roots()
    );
}

/// The rename is NOT aliased: an old `repo-root:` fails to load, and the message says what replaced
/// it. `deny_unknown_fields` alone would say only "unknown field", leaving a teammate to guess that
/// the value simply became a list.
///
/// BITES: delete the `repo_root` field and the error stops naming `repo-roots`.
#[test]
fn repo_root_singular_errors_with_the_migration() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("clones");
    std::fs::create_dir_all(&root).unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, format!("repo-root: {}\n", root.display())).unwrap();
    let err = format!("{:#}", load_from(&path).unwrap_err());
    assert!(err.contains("repo-roots"), "must name the NEW key: {err}");
    assert!(err.contains("repo-root"), "must name the OLD key: {err}");
    assert!(
        !err.contains("unknown field"),
        "the generic deny_unknown_fields message is what this replaces: {err}"
    );
}

/// The migration error fires on ANY value shape, including the list a hurried reader might write
/// under the old key. The value is consumed and discarded before the error, so the message is about
/// the key.
#[test]
fn repo_root_singular_errors_even_when_given_a_list() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "repo-root: [/tmp]\n").unwrap();
    let err = format!("{:#}", load_from(&path).unwrap_err());
    assert!(err.contains("repo-roots"), "must name the NEW key: {err}");
}

// ---- min-enrichment ------------------------------------------------------------------------------

#[test]
fn min_enrichment_defaults_to_half() {
    assert_eq!(Config::default().min_enrichment(), DEFAULT_MIN_ENRICHMENT);
    assert_eq!(DEFAULT_MIN_ENRICHMENT, 0.5, "the documented default is 50%");
}

#[test]
fn min_enrichment_override_from_clyde_yml() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "min-enrichment: 0.8\n").unwrap();
    assert_eq!(load_from(&path).unwrap().min_enrichment(), 0.8);
}

/// `min-enrichment: 50` (meaning 50%) is the confusion worth failing on: silently accepted, it
/// would configure a floor no window can ever meet and warn on every single run.
#[test]
fn min_enrichment_rejects_a_percent_by_name() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "min-enrichment: 50\n").unwrap();
    let err = format!("{:#}", load_from(&path).unwrap_err());
    assert!(err.contains("min-enrichment"), "must name the key the user set: {err}");
    assert!(err.contains("0.5 means 50%"), "must say what the units are: {err}");
}

#[test]
fn min_enrichment_rejects_a_negative_fraction() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "min-enrichment: -0.1\n").unwrap();
    assert!(load_from(&path).is_err());
}

#[test]
fn min_enrichment_rejects_the_typo_form() {
    // deny_unknown_fields: `min-enrichement` must fail LOUD rather than silently keeping 0.5.
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "min-enrichement: 0.8\n").unwrap();
    assert!(
        load_from(&path).is_err(),
        "deny_unknown_fields should reject `min-enrichement`"
    );
}

#[test]
fn xdg_config_dir_honors_env_and_falls_back() {
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("XDG_CONFIG_HOME").ok();

    let dir = tempfile::TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
    assert_eq!(xdg_config_dir().as_deref(), Some(dir.path()));

    unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
    assert!(xdg_config_dir().unwrap().ends_with(".config"));

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
    }
    drop(guard);
}
