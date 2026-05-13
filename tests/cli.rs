//! End-to-end CLI tests. Exercise the wired-up binary via assert_cmd.

use std::path::Path;

use assert_cmd::Command;

fn write(dir: &Path, name: &str, body: &str) {
    std::fs::write(dir.join(name), body).unwrap();
}

fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "a.jsonnet",
        "(import 'b.libsonnet') + (import 'c.libsonnet')",
    );
    write(tmp.path(), "b.libsonnet", "import 'd.libsonnet'");
    write(tmp.path(), "c.libsonnet", "{}");
    write(tmp.path(), "d.libsonnet", "{}");
    tmp
}

fn run(tmp: &tempfile::TempDir) -> Command {
    let cache = tmp.path().join("cache.json");
    let mut cmd = Command::cargo_bin("jsondepnet").unwrap();
    cmd.env("JSONDEPNET_CACHE_FILE", &cache);
    cmd.env("JSONDEPNET_ROOT_DIR", tmp.path());
    cmd.current_dir(tmp.path());
    cmd
}

#[test]
fn cache_all_then_list_forward() {
    let tmp = fixture();
    run(&tmp).args(["cache", "--all"]).assert().success();
    let out = run(&tmp)
        .args(["list", "--skip-update-cache", "a.jsonnet"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let mut lines: Vec<&str> = stdout.lines().collect();
    lines.sort();
    assert_eq!(lines, vec!["b.libsonnet", "c.libsonnet", "d.libsonnet"]);
}

#[test]
fn list_reverse() {
    let tmp = fixture();
    run(&tmp).args(["cache", "--all"]).assert().success();
    let out = run(&tmp)
        .args(["list", "-r", "--skip-update-cache", "d.libsonnet"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let mut lines: Vec<&str> = stdout.lines().collect();
    lines.sort();
    assert_eq!(lines, vec!["a.jsonnet", "b.libsonnet"]);
}

#[test]
fn list_no_libsonnet_filters() {
    let tmp = fixture();
    run(&tmp).args(["cache", "--all"]).assert().success();
    let out = run(&tmp)
        .args(["list", "-L", "--skip-update-cache", "a.jsonnet"])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.trim().is_empty(), "expected empty, got: {stdout:?}");
}

#[test]
fn tree_json_is_valid() {
    let tmp = fixture();
    run(&tmp).args(["cache", "--all"]).assert().success();
    let out = run(&tmp)
        .args(["tree", "-j", "--skip-update-cache", "a.jsonnet"])
        .assert()
        .success();
    let stdout = out.get_output().stdout.clone();

    #[derive(serde::Deserialize)]
    struct Out {
        roots: Vec<Node>,
    }
    #[derive(serde::Deserialize)]
    struct Node {
        path: String,
        children: Vec<Node>,
    }
    let parsed: Out = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(parsed.roots.len(), 1);
    assert_eq!(parsed.roots[0].path, "a.jsonnet");
    assert_eq!(parsed.roots[0].children.len(), 2);
}

#[test]
fn list_null_separated_roundtrips() {
    let tmp = fixture();
    run(&tmp).args(["cache", "--all"]).assert().success();
    let out = run(&tmp)
        .args(["list", "-0", "--skip-update-cache", "a.jsonnet"])
        .assert()
        .success();
    let bytes = out.get_output().stdout.clone();
    let mut parts: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    if parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    let mut as_strs: Vec<&str> = parts
        .iter()
        .map(|p| std::str::from_utf8(p).unwrap())
        .collect();
    as_strs.sort();
    assert_eq!(as_strs, vec!["b.libsonnet", "c.libsonnet", "d.libsonnet"]);
}

#[test]
fn tree_text_null_rejected() {
    let tmp = fixture();
    run(&tmp).args(["cache", "--all"]).assert().success();
    run(&tmp)
        .args(["tree", "-0", "--skip-update-cache", "a.jsonnet"])
        .assert()
        .failure();
}

#[test]
fn cache_replace_drops_other_entries() {
    let tmp = fixture();
    run(&tmp).args(["cache", "--all"]).assert().success();
    let cache_path = tmp.path().join("cache.json");
    let before = std::fs::read_to_string(&cache_path).unwrap();
    assert!(before.contains("a.jsonnet"));
    assert!(before.contains("d.libsonnet"));

    run(&tmp)
        .args(["cache", "--replace", "d.libsonnet"])
        .assert()
        .success();
    let after = std::fs::read_to_string(&cache_path).unwrap();
    assert!(after.contains("d.libsonnet"));
    assert!(!after.contains("a.jsonnet"));
}

#[test]
fn path_style_absolute() {
    let tmp = fixture();
    run(&tmp).args(["cache", "--all"]).assert().success();
    let out = run(&tmp)
        .args([
            "list",
            "--skip-update-cache",
            "--path-style",
            "absolute",
            "a.jsonnet",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let root_canon = tmp.path().canonicalize().unwrap();
    for line in stdout.lines() {
        let p = std::path::Path::new(line);
        assert!(p.is_absolute(), "expected absolute path, got: {line:?}");
        assert!(
            line.starts_with(&root_canon.display().to_string()),
            "expected line under root, got: {line:?}"
        );
    }
}
