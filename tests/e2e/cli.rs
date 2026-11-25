//! CLI surface: flags, --help/--version, usage errors and exit codes.

use super::*;

// --- CLI flag tests ---------------------------------------------------------

#[test]
fn cli_version_short_and_long() {
    for flag in ["-v", "--version"] {
        let r = run_jse("", &[flag]);
        assert_eq!(r.code, 0, "{flag}: stderr:\n{}", r.stderr);
        assert!(
            r.stdout.starts_with("jse v"),
            "{flag}: stdout:\n{}",
            r.stdout
        );
        assert!(r.stderr.is_empty(), "{flag}: stderr:\n{}", r.stderr);
    }
}

#[test]
fn cli_help_short_and_long() {
    for flag in ["-h", "--help"] {
        let r = run_jse("", &[flag]);
        assert_eq!(r.code, 0, "{flag}: stderr:\n{}", r.stderr);
        assert!(
            r.stderr.contains("Usage: jse"),
            "{flag}: usage missing:\n{}",
            r.stderr
        );
        assert!(
            r.stderr.contains("-v, --version"),
            "{flag}: flags missing:\n{}",
            r.stderr
        );
    }
}

#[test]
fn cli_bad_flags() {
    let r = run_jse("", &["--bogus"]);
    assert_eq!(r.code, 2, "stderr:\n{}", r.stderr);
    assert!(
        r.stderr.contains("unrecognized option '--bogus'"),
        "stderr:\n{}",
        r.stderr
    );

    let r = run_jse("", &["-z"]);
    assert_eq!(r.code, 2, "stderr:\n{}", r.stderr);
    assert!(
        r.stderr.contains("invalid option -- 'z'"),
        "stderr:\n{}",
        r.stderr
    );

    let r = run_jse("", &["--force=false", "cfg.json"]);
    assert_eq!(r.code, 2, "stderr:\n{}", r.stderr);
    assert!(
        r.stderr
            .contains("option '--force' doesn't allow an argument"),
        "stderr:\n{}",
        r.stderr
    );
}

#[test]
fn cli_long_schema() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    write_file(&schema, MINI_SCHEMA);
    let cfg = dir.path().join("cfg.json");
    write_file(&cfg, r#"{"name": "x"}"#);
    let cfg_arg = arg(&cfg);
    let schema_arg = arg(&schema);
    let schema_flag = format!("--schema={schema_arg}");

    let r = run_jse("\n\n", &[schema_flag.as_str(), cfg_arg.as_str(), "-a"]);
    assert_eq!(
        r.code, 0,
        "exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    assert!(r.stdout.contains("Saved to"), "stdout:\n{}", r.stdout);
    assert_eq!(read_json(&cfg)["$schema"], json!(schema_arg));
}

#[test]
fn cli_double_dash_operand() {
    let dir = mktemp();
    let cache = mktemp();
    write_file(&dir.path().join("mini.schema.json"), MINI_SCHEMA);
    write_file(
        &dir.path().join("-dash.json"),
        r#"{"$schema": "mini.schema.json", "name": "x"}"#,
    );
    let (k, v) = cache_env(&cache);

    let r = run_jse_env(
        dir.path(),
        "\n\n",
        &["-a", "--", "-dash.json"],
        &[(k.as_str(), v.as_str())],
        Duration::from_secs(60),
    );
    assert_eq!(
        r.code, 0,
        "exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    assert!(r.stdout.contains("Saved to"), "stdout:\n{}", r.stdout);
}

/// CLI rejection paths run as plain processes: a missing config argument is
/// a usage error (exit 2), an http:// config path is refused, a config that
/// fails to parse is a runtime error (exit 1), and --pick stays fatal when
/// the schema catalog cannot be loaded.
#[test]
fn cli_rejection_paths() {
    // no positional argument → usage on stderr, exit 2
    let r = run_jse("", &[]);
    assert_eq!(r.code, 2, "stderr:\n{}", r.stderr);
    assert!(r.stderr.contains("no config file"), "stderr:\n{}", r.stderr);
    assert!(r.stderr.contains("Usage: jse"), "stderr:\n{}", r.stderr);

    // an http:// config path is a usage error, not a fetch
    let r = run_jse("", &["-a", "http://example.com/cfg.json"]);
    assert_eq!(r.code, 2, "stderr:\n{}", r.stderr);
    assert!(
        r.stderr.contains("config must be a local file path"),
        "stderr:\n{}",
        r.stderr
    );

    // a config that does not parse is a runtime error
    let dir = mktemp();
    let broken = dir.path().join("broken.json");
    write_file(&broken, "{\"name\": ");
    let broken_arg = arg(&broken);
    let r = run_jse("", &["-a", broken_arg.as_str()]);
    assert_eq!(r.code, 1, "stderr:\n{}", r.stderr);
    assert!(r.stderr.contains("parse config"), "stderr:\n{}", r.stderr);

    // --pick with an unreachable catalog is fatal (the run cannot open)
    let dir = mktemp();
    let cfg = dir.path().join("cfg.json");
    write_file(&cfg, r#"{"name": "x"}"#);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let cache_dir = arg(cache.path());
    let r = run_jse_env(
        repo_root(),
        "",
        &["-a", "--pick", cfg_arg.as_str()],
        &[
            ("JSE_CACHE_DIR", cache_dir.as_str()),
            ("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json"),
        ],
        Duration::from_secs(30),
    );
    assert_ne!(r.code, 0, "stderr:\n{}", r.stderr);
    assert!(
        r.stderr.contains("load schema catalog"),
        "stderr:\n{}",
        r.stderr
    );
}
