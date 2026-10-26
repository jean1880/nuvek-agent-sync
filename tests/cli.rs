//! End-to-end runs of the `agent-sync` binary, each against its own temporary HOME. The source is
//! a copy of the public template checkout in `tests/fixtures/agent-config`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/agent-config");
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/no-overlay.txt");

/// A temporary HOME, removed on drop.
struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Home {
        let d = std::env::temp_dir().join(format!(
            "agent-sync-it-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Home(d)
    }
    fn path(&self) -> &Path {
        &self.0
    }
    /// The default source location, `~/git/agent-config`, seeded from the fixture.
    fn seed_base(&self) -> PathBuf {
        let base = self.0.join("git/agent-config");
        copy_tree(Path::new(FIXTURE), &base);
        base
    }
    fn run(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_agent-sync"));
        cmd.args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self.0)
            .env("XDG_STATE_HOME", self.0.join(".local/state"))
            .env("NUVEK_LOG_SINK", "stderr")
            // Never reach a real Claude CLI: any registration attempt fails loudly.
            .env("AGENT_SYNC_CLAUDE_BIN", "/bin/false");
        cmd.output().unwrap()
    }
    fn code(&self, args: &[&str]) -> i32 {
        let o = self.run(args);
        o.status.code().unwrap_or(-1)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), dst).unwrap();
        }
    }
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn a_fresh_home_with_no_tool_files_is_drift_not_an_error() {
    let home = Home::new("fresh");
    home.seed_base();
    let o = home.run(&["check"]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(6), "{out}");
    for missing in [
        "file ~/.claude.json: missing",
        "file ~/.claude/settings.json: missing",
        "file ~/.codex/config.toml: missing",
        "file ~/.gemini/antigravity-cli/settings.json: missing",
    ] {
        assert!(out.contains(missing), "{missing} not reported:\n{out}");
    }
}

#[test]
fn apply_then_check_is_clean_and_a_hand_edit_is_drift() {
    let home = Home::new("clean");
    home.seed_base();
    let o = home.run(&["apply", "--force"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert_eq!(home.code(&["check"]), 0);
    std::fs::write(home.path().join(".codex/AGENTS.md"), "hand edit\n").unwrap();
    assert_eq!(home.code(&["check"]), 6);
}

#[test]
fn an_invalid_secret_surface_file_exits_7() {
    let home = Home::new("surfaces");
    home.seed_base();
    let o = Command::new(env!("CARGO_BIN_EXE_agent-sync"))
        .arg("check")
        .env("HOME", home.path())
        .env("NUVEK_LOG_SINK", "stderr")
        .env(
            "NUVEK_SECRET_SURFACES_FILE",
            home.path().join("does-not-exist.toml"),
        )
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(7), "{}", text(&o));
}

/// Every file and link under HOME except the source checkouts and the backups, with HOME
/// spelled `$HOME`: the complete footprint of an apply.
fn footprint(home: &Path) -> String {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let rel = p.strip_prefix(root).unwrap().display().to_string();
            if rel == "git" || rel == "src" || rel.ends_with(".agent-sync.bak") || rel == ".local" {
                continue;
            }
            let ft = std::fs::symlink_metadata(&p).unwrap().file_type();
            let home = root.display().to_string();
            if ft.is_symlink() {
                let t = std::fs::read_link(&p).unwrap().display().to_string();
                out.push(format!("== {rel} -> {}\n", t.replace(&home, "$HOME")));
            } else if ft.is_dir() {
                walk(root, &p, out);
            } else {
                let body = std::fs::read_to_string(&p).unwrap();
                out.push(format!("== {rel}\n{}\n", body.replace(&home, "$HOME")));
            }
        }
    }
    let mut out = Vec::new();
    walk(home, home, &mut out);
    out.concat()
}

/// The pre-overlay behaviour, pinned: link targets and rendered files for a source with no
/// overlay. Regenerate with `UPDATE_GOLDEN=1 cargo test` only for an intended output change.
#[test]
fn golden_no_overlay_output() {
    let home = Home::new("golden");
    home.seed_base();
    let o = home.run(&["apply", "--force"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let got = footprint(home.path());
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(Path::new(GOLDEN).parent().unwrap()).unwrap();
        std::fs::write(GOLDEN, &got).unwrap();
    }
    let want = std::fs::read_to_string(GOLDEN).unwrap();
    assert!(got == want, "footprint differs from {GOLDEN}:\n{got}");
    // The load-bearing lines, spelled out.
    assert!(want.contains("== .claude/plugins/nuvek-skills -> $HOME/git/agent-config\n"));
    assert!(want.contains(
        "== .claude/CLAUDE.md\n# Claude Global Context\n@$HOME/git/agent-config/rules/core.md\n@$HOME/git/agent-config/rules/claude.md\n"
    ));
    assert!(want.contains(
        "== .codex/AGENTS.md\n<!-- GENERATED by agent-sync from ~/git/agent-config -->\n"
    ));
    assert!(want.contains(
        "<!-- GENERATED by agent-sync from ~/git/agent-config/sync/agents/rust-reviewer.md — edit the source, not this file -->"
    ));
}

const REFERENCE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/golden/reference-custom-source.txt"
);

/// Tool files that already exist, with settings agent-sync must leave alone.
fn seed_existing_config(home: &Path) {
    write(
        &home.join(".claude.json"),
        "{\n  \"numStartups\": 3,\n  \"mcpServers\": {}\n}\n",
    );
    write(
        &home.join(".claude/settings.json"),
        "{\n  \"model\": \"opus\",\n  \"statusLine\": {\"type\": \"command\", \"command\": \"status.sh\"}\n}\n",
    );
    write(
        &home.join(".codex/config.toml"),
        "model = \"gpt-5\"\n\n[profiles.fast]\nmodel = \"mini\"\n",
    );
    write(
        &home.join(".gemini/antigravity-cli/settings.json"),
        "{\n  \"theme\": \"dark\",\n  \"permissions\": {\"allow\": [\"command(make)\"]}\n}\n",
    );
}

/// Parity with a reference footprint, for a source outside the default location and a machine
/// that already has tool configuration. The footprint is recorded from a reference binary
/// (`REFERENCE_AGENT_SYNC=<binary> UPDATE_GOLDEN=1`).
///
/// Two names are normalised. The generated headers name the source location, and the
/// comparison writes that as `~/git/agent-config`. A reference binary that links the plugin
/// under a fixed name passes it as `REFERENCE_PLUGIN_LINK=<name>`, and it is recorded as the
/// fixture's `my-skills`. Every other byte is the reference's.
#[test]
fn matches_the_reference_for_a_custom_source_and_an_existing_config() {
    let home = Home::new("reference");
    let src = home.path().join("src/my-config");
    copy_tree(Path::new(FIXTURE), &src);
    write(
        &src.join(".claude-plugin/plugin.json"),
        "{\"name\": \"my-skills\"}\n",
    );
    seed_existing_config(home.path());
    let dir = src.to_str().unwrap();
    let mut cmd = match std::env::var_os("REFERENCE_AGENT_SYNC") {
        Some(bin) => Command::new(bin),
        None => Command::new(env!("CARGO_BIN_EXE_agent-sync")),
    };
    let o = cmd
        .args(["--dir", dir, "apply", "--force"])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home.path())
        .env("XDG_STATE_HOME", home.path().join(".local/state"))
        .env("NUVEK_LOG_SINK", "stderr")
        .env("AGENT_SYNC_CLAUDE_BIN", "/bin/false")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let mut got = footprint(home.path()).replace(
        "GENERATED by agent-sync from ~/src/my-config",
        "GENERATED by agent-sync from ~/git/agent-config",
    );
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        if std::env::var_os("REFERENCE_AGENT_SYNC").is_none() {
            panic!("regenerate {REFERENCE} from a reference binary: set REFERENCE_AGENT_SYNC");
        }
        let recorded = match std::env::var("REFERENCE_PLUGIN_LINK") {
            Ok(fixed) if !fixed.is_empty() => got
                .replace(&format!("/{fixed} -> "), "/my-skills -> ")
                .replace(&format!("\"{fixed}\""), "\"my-skills\""),
            _ => got.clone(),
        };
        std::fs::write(REFERENCE, &recorded).unwrap();
        got = recorded;
    }
    let want = std::fs::read_to_string(REFERENCE).unwrap();
    assert!(got == want, "footprint differs from {REFERENCE}:\n{got}");
    // Settings agent-sync does not own survived.
    assert!(want.contains("\"numStartups\": 3"));
    assert!(want.contains("[profiles.fast]"));
    assert!(want.contains("\"command(make)\""));
    // What this binary wrote is in sync with itself.
    if std::env::var_os("REFERENCE_AGENT_SYNC").is_none() {
        assert_eq!(home.code(&["--dir", dir, "check"]), 0);
    }
}

// ---------- exit codes ----------

/// A PATH with no `pgrep`: agy_running() fails closed and reports agy as running.
fn without_pgrep(home: &Home, args: &[&str]) -> i32 {
    let empty = home.path().join("empty-bin");
    std::fs::create_dir_all(&empty).unwrap();
    Command::new(env!("CARGO_BIN_EXE_agent-sync"))
        .args(args)
        .env_clear()
        .env("PATH", &empty)
        .env("HOME", home.path())
        .env("XDG_STATE_HOME", home.path().join(".local/state"))
        .env("NUVEK_LOG_SINK", "stderr")
        .env("AGENT_SYNC_CLAUDE_BIN", "/bin/false")
        .output()
        .unwrap()
        .status
        .code()
        .unwrap_or(-1)
}

#[test]
fn each_exit_code_has_its_cause() {
    let home = Home::new("codes");
    // 4: no source.
    assert_eq!(home.code(&["check"]), 4);
    home.seed_base();
    // 6: drift on a fresh machine; 0 once applied.
    assert_eq!(home.code(&["check"]), 6);
    // 5: agy may be running (pgrep cannot answer), for apply and for promote --drop alike.
    assert_eq!(without_pgrep(&home, &["apply"]), 5);
    assert_eq!(home.code(&["apply", "--force"]), 0);
    assert_eq!(home.code(&["check"]), 0);
    let agy = home.path().join(".gemini/antigravity-cli/settings.json");
    let mut v = json(&agy);
    v["permissions"]["allow"]
        .as_array_mut()
        .unwrap()
        .push("command(make)".into());
    write(&agy, &serde_json::to_string_pretty(&v).unwrap());
    assert_eq!(
        without_pgrep(&home, &["promote", "--drop", "command(make)"]),
        5
    );
    // 2: usage, and a writing command refused with an overlay.
    assert_eq!(home.code(&["check", "--no-such-flag"]), 2);
    let ov = home.path().join("git/private");
    std::fs::create_dir_all(&ov).unwrap();
    assert_eq!(home.code(&["--overlay", ov.to_str().unwrap(), "import"]), 2);
    // 4: an overlay that does not exist.
    assert_eq!(
        home.code(&["--overlay", "/nonexistent/overlay", "check"]),
        4
    );
}

// ---------- safety ----------

#[test]
fn two_concurrent_applies_serialise_and_both_converge() {
    let (home, _base, overlay) = layered("concurrent");
    skill(&overlay, "private-only", "x");
    let ov = overlay.to_str().unwrap().to_string();
    // The test holds the state lock itself: a barrier both runs must wait at.
    let state = home.path().join(".local/state/agent-sync");
    std::fs::create_dir_all(&state).unwrap();
    let barrier = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join("lock"))
        .unwrap();
    barrier.lock().unwrap();
    let spawn = || {
        Command::new(env!("CARGO_BIN_EXE_agent-sync"))
            .args(["--overlay", &ov, "apply", "--force"])
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", home.path())
            .env("XDG_STATE_HOME", home.path().join(".local/state"))
            .env("NUVEK_LOG_SINK", "stderr")
            .env("AGENT_SYNC_CLAUDE_BIN", "/bin/false")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    };
    let (mut a, mut b) = (spawn(), spawn());
    // Handshake, no sleep: wait until the kernel lists both workers as blocked waiters on a
    // flock (`/proc/locks` marks a waiter with `->`). Each then sits at the lock.
    let waiting = |pid: u32| {
        std::fs::read_to_string("/proc/locks")
            .unwrap_or_default()
            .lines()
            .any(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                f.get(1) == Some(&"->")
                    && f.get(2) == Some(&"FLOCK")
                    && f.get(5) == Some(&pid.to_string().as_str())
            })
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !(waiting(a.id()) && waiting(b.id())) {
        assert!(
            std::time::Instant::now() < deadline,
            "workers never reached the lock"
        );
        assert!(a.try_wait().unwrap().is_none() && b.try_wait().unwrap().is_none());
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Before the lock is released: no tree swapped in, and no candidate generation built.
    assert!(std::fs::symlink_metadata(state.join("resolved")).is_err());
    let candidates = std::fs::read_dir(state.join("generations"))
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(
        candidates, 0,
        "a candidate was built before the lock was held"
    );
    drop(barrier);
    assert!(a.wait().unwrap().success());
    assert!(b.wait().unwrap().success());
    assert_eq!(home.code(&["--overlay", &ov, "check"]), 0);
    // One run swapped the tree in; the other, serialised after it, found it current.
    let marked = std::fs::read_dir(state.join("generations"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().join(".agent-sync-generation").is_file())
        .count();
    assert_eq!(marked, 1);
    // No temporary file was left behind by either run.
    let leftovers: Vec<_> = std::fs::read_dir(home.path().join(".claude"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".agent-sync-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn planted_backup_and_temp_links_never_reach_their_targets() {
    let home = Home::new("planted");
    home.seed_base();
    assert_eq!(home.code(&["apply", "--force"]), 0);
    let victim = home.path().join("victim.txt");
    write(&victim, "untouched\n");
    let codex = home.path().join(".codex/AGENTS.md");
    for planted in [".agent-sync.bak", ".agent-sync.tmp"] {
        let p = PathBuf::from(format!("{}{planted}", codex.display()));
        let _ = std::fs::remove_file(&p);
        std::os::unix::fs::symlink(&victim, &p).unwrap();
    }
    write(&codex, "hand edit\n");
    assert_eq!(home.code(&["apply", "--force"]), 0);
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched\n");
    // A symlinked tool directory that leads outside the managed roots is refused.
    let elsewhere = home.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::remove_dir_all(home.path().join(".gemini/config/agents")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, home.path().join(".gemini/config/agents")).unwrap();
    assert_eq!(home.code(&["apply", "--force"]), 6);
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn diagnostics_never_echo_a_credential() {
    let home = Home::new("redact");
    let base = home.seed_base();
    assert_eq!(home.code(&["apply", "--force"]), 0);
    // Built at runtime so no scanner sees a literal.
    let token = format!("ghp_{}", "Zq9Xw8Vu7Ts6Rq5Po4Nm3Lk2Jh1Gf0Ed9Cb8A");
    let agent = home.path().join(".claude/agents/rust-reviewer.md");
    let mut text = std::fs::read_to_string(&agent).unwrap();
    text.push_str(&format!("leaked {token}\n"));
    write(&agent, &text);
    let o = home.run(&["check", "--verbose"]);
    assert_eq!(o.status.code(), Some(6));
    let out = crate::text(&o);
    assert!(out.contains("content differs"), "{out}");
    assert!(!out.contains(&token), "{out}");
    // A parser error quotes the offending line: it is scrubbed too.
    write(
        &base.join("sync/mcp.toml"),
        &format!("[servers.x]\ncommand = \"{token}\nagents = [\n"),
    );
    let o = home.run(&["check"]);
    let out = crate::text(&o);
    assert_eq!(o.status.code(), Some(6), "{out}");
    assert!(out.contains("mcp.toml"), "{out}");
    assert!(!out.contains(&token), "{out}");
    // A credential used as a server name reaches apply's audit log (stderr here) and its
    // error: both are scrubbed, and the write is refused.
    write(
        &base.join("sync/mcp.toml"),
        &format!("[servers.\"{token}\"]\ncommand = \"x\"\nagents = [\"codex\"]\n"),
    );
    let o = home.run(&["apply", "--force"]);
    let out = crate::text(&o);
    assert_eq!(o.status.code(), Some(6), "{out}");
    assert!(out.contains("codex mcp"), "the audit line ran: {out}");
    assert!(!out.contains(&token), "{out}");
    let codex = std::fs::read_to_string(home.path().join(".codex/config.toml")).unwrap();
    assert!(!codex.contains(&token));
}

/// A stub `claude` that records each call's arguments, one line per call.
fn recording_cli(home: &Home) -> (PathBuf, PathBuf) {
    let calls = home.path().join("claude-calls.log");
    let stub = home.path().join("bin/claude");
    write(
        &stub,
        &format!("#!/bin/sh\necho \"$@\" >> '{}'\n", calls.display()),
    );
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    (stub, calls)
}

#[test]
fn prune_never_puts_a_secret_shaped_server_name_on_the_command_line() {
    let home = Home::new("prune-secret");
    home.seed_base();
    let token = format!("ghp_{}", "Zq9Xw8Vu7Ts6Rq5Po4Nm3Lk2Jh1Gf0Ed9Cb8A");
    write(
        &home.path().join(".claude.json"),
        &format!(
            "{{\"mcpServers\": {{\"{token}\": {{\"command\": \"x\"}}, \"old\": {{\"command\": \"y\"}}}}}}\n"
        ),
    );
    let (stub, calls) = recording_cli(&home);
    let o = Command::new(env!("CARGO_BIN_EXE_agent-sync"))
        .args(["apply", "--force", "--prune"])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home.path())
        .env("XDG_STATE_HOME", home.path().join(".local/state"))
        .env("NUVEK_LOG_SINK", "stderr")
        .env("AGENT_SYNC_CLAUDE_BIN", &stub)
        .output()
        .unwrap();
    let out = text(&o);
    assert!(out.contains("not pruned"), "{out}");
    assert!(!out.contains(&token), "{out}");
    let log = std::fs::read_to_string(&calls).unwrap_or_default();
    // The ordinary stale server is pruned; the secret-named one never reaches argv.
    assert!(log.contains("mcp remove -s user old"), "{log}");
    assert!(!log.contains(&token), "{log}");
}

#[test]
fn claude_mcp_is_never_run_against_a_symlinked_claude_json() {
    let home = Home::new("claude-json-link");
    let base = home.seed_base();
    write(
        &base.join("sync/mcp.toml"),
        "[servers.files]\ncommand = \"files-mcp\"\nagents = [\"claude\"]\n",
    );
    let real = home.path().join("elsewhere/claude.json");
    write(&real, "{\"mcpServers\": {}}\n");
    std::os::unix::fs::symlink(&real, home.path().join(".claude.json")).unwrap();
    // A stub CLI that records every call.
    let calls = home.path().join("claude-calls.log");
    let stub = home.path().join("bin/claude");
    write(
        &stub,
        &format!("#!/bin/sh\necho \"$@\" >> '{}'\n", calls.display()),
    );
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let o = Command::new(env!("CARGO_BIN_EXE_agent-sync"))
        .args(["apply", "--force"])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home.path())
        .env("XDG_STATE_HOME", home.path().join(".local/state"))
        .env("NUVEK_LOG_SINK", "stderr")
        .env("AGENT_SYNC_CLAUDE_BIN", &stub)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(6), "{}", text(&o));
    assert!(text(&o).contains("is a symlink"), "{}", text(&o));
    assert!(!calls.exists(), "the claude CLI was invoked");
    // Control: with a real file, the stub is called.
    std::fs::remove_file(home.path().join(".claude.json")).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_agent-sync"))
        .args(["apply", "--force"])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home.path())
        .env("XDG_STATE_HOME", home.path().join(".local/state"))
        .env("NUVEK_LOG_SINK", "stderr")
        .env("AGENT_SYNC_CLAUDE_BIN", &stub)
        .output()
        .unwrap();
    assert!(calls.exists(), "{}", text(&o));
}

#[test]
fn a_failure_midway_through_the_agents_stage_still_records_what_it_wrote() {
    let home = Home::new("mid-agents");
    home.seed_base();
    // The second agent (sorted) cannot be written: a directory sits at its path.
    std::fs::create_dir_all(home.path().join(".claude/agents/rust-reviewer.md")).unwrap();
    assert_eq!(home.code(&["apply", "--force"]), 6);
    let files: Vec<String> = manifest(&home)["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap().to_string())
        .collect();
    assert!(
        files
            .iter()
            .any(|f| f.ends_with(".claude/agents/plan-reviewer.md")),
        "{files:?}"
    );
    assert!(!files.iter().any(|f| f.ends_with("rust-reviewer.md")));
}

// ---------- ownership ----------

fn manifest(home: &Home) -> serde_json::Value {
    json(&home.path().join(".local/state/agent-sync/managed.json"))
}

#[test]
fn the_first_run_deletes_nothing_and_unrecorded_files_are_left_alone() {
    let home = Home::new("first-run");
    home.seed_base();
    // Left by an earlier tool: a generated-looking agent with no source, and a skill link.
    let orphan = home.path().join(".claude/agents/old-agent.md");
    write(
        &orphan,
        "---\nname: old-agent\ndescription: d\nmodel: sonnet\n---\n<!-- GENERATED by agent-sync from ~/x/sync/agents/old-agent.md — edit the source, not this file -->\nbody\n",
    );
    let old_link = home.path().join(".codex/skills/old-skill");
    std::fs::create_dir_all(old_link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(
        home.path().join("git/agent-config/skills/old-skill"),
        &old_link,
    )
    .unwrap();
    let o = home.run(&["apply", "--force"]);
    // The unrecorded agent stays, so apply cannot converge: drift until removed by hand.
    assert_eq!(o.status.code(), Some(6), "{}", text(&o));
    assert!(
        text(&o).contains("not recorded in managed.json"),
        "{}",
        text(&o)
    );
    assert!(orphan.exists());
    assert!(std::fs::symlink_metadata(&old_link).is_ok());
    assert_eq!(home.code(&["check"]), 6);
    std::fs::remove_file(&orphan).unwrap();
    assert_eq!(home.code(&["check"]), 0);
    // The manifest lists what the run made, and not the foreign link.
    let m = manifest(&home);
    assert!(m["links"]
        .as_object()
        .unwrap()
        .keys()
        .any(|k| k.ends_with(".codex/skills/delegation")));
    assert!(!m["links"]
        .as_object()
        .unwrap()
        .keys()
        .any(|k| k.ends_with("old-skill")));
}

#[test]
fn a_hand_changed_link_or_agent_is_never_pruned() {
    let (home, base, overlay) = layered("hand-changed");
    skill(&overlay, "gone-soon", "temporary");
    let agent = std::fs::read_to_string(base.join("sync/agents/skill-auditor.md")).unwrap();
    write(
        &overlay.join("sync/agents/private-agent.md"),
        &agent.replace("name: skill-auditor", "name: private-agent"),
    );
    let ov = overlay.to_str().unwrap();
    assert_eq!(home.code(&["--overlay", ov, "apply", "--force"]), 0);
    // Retarget the link by hand, and replace the generated agent with the user's own file.
    let link = home.path().join(".codex/skills/gone-soon");
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink("/somewhere/else", &link).unwrap();
    let mine = home.path().join(".claude/agents/private-agent.md");
    write(
        &mine,
        "---\nname: private-agent\ndescription: mine\nmodel: sonnet\n---\nhand written\n",
    );
    std::fs::remove_dir_all(overlay.join("skills/gone-soon")).unwrap();
    std::fs::remove_file(overlay.join("sync/agents/private-agent.md")).unwrap();
    let o = home.run(&["--overlay", ov, "apply", "--force"]);
    assert_eq!(o.status.code(), Some(6), "{}", text(&o));
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        PathBuf::from("/somewhere/else")
    );
    // A file without the generated header is someone else's: apply refuses, never deletes.
    assert!(text(&o).contains("is not generated"), "{}", text(&o));
    assert!(std::fs::read_to_string(&mine)
        .unwrap()
        .contains("hand written"));
}

#[test]
fn a_failed_stage_still_records_what_was_created() {
    let home = Home::new("partial");
    home.seed_base();
    // Antigravity's settings are unparseable, so apply stops at the permissions stage.
    write(
        &home.path().join(".gemini/antigravity-cli/settings.json"),
        "{ not json",
    );
    assert_eq!(home.code(&["apply", "--force"]), 6);
    let m = manifest(&home);
    assert!(!m["links"].as_object().unwrap().is_empty());
    assert!(m["files"].as_array().unwrap().iter().any(|f| f
        .as_str()
        .unwrap()
        .ends_with(".claude/agents/rust-reviewer.md")));
}

#[test]
fn a_corrupt_manifest_stops_the_overlay_before_it_is_published() {
    let (home, _base, overlay) = layered("corrupt-overlay");
    let ov = overlay.to_str().unwrap();
    assert_eq!(home.code(&["--overlay", ov, "apply", "--force"]), 0);
    let live = std::fs::read_link(resolved(&home)).unwrap();
    write(
        &home.path().join(".local/state/agent-sync/managed.json"),
        "garbage",
    );
    skill(&overlay, "new-skill", "would change the tree");
    let o = home.run(&["--overlay", ov, "apply", "--force"]);
    assert_eq!(o.status.code(), Some(6), "{}", text(&o));
    assert_eq!(std::fs::read_link(resolved(&home)).unwrap(), live);
    assert!(!resolved(&home).join("skills/new-skill").exists());
}

#[test]
fn a_corrupt_manifest_says_how_to_recover() {
    let home = Home::new("corrupt");
    home.seed_base();
    assert_eq!(home.code(&["apply", "--force"]), 0);
    write(
        &home.path().join(".local/state/agent-sync/managed.json"),
        "garbage",
    );
    let o = home.run(&["apply", "--force"]);
    assert_eq!(o.status.code(), Some(6));
    assert!(
        text(&o).contains("delete it to start fresh"),
        "{}",
        text(&o)
    );
}

#[test]
fn a_resolved_path_agent_sync_did_not_make_is_refused() {
    let (home, _base, overlay) = layered("foreign-resolved");
    let ov = overlay.to_str().unwrap();
    let state = home.path().join(".local/state/agent-sync");
    std::fs::create_dir_all(&state).unwrap();
    std::os::unix::fs::symlink("/elsewhere", state.join("resolved")).unwrap();
    let o = home.run(&["--overlay", ov, "apply", "--force"]);
    assert_eq!(o.status.code(), Some(6));
    assert!(text(&o).contains("not at a generation"), "{}", text(&o));
    std::fs::remove_file(state.join("resolved")).unwrap();
    std::fs::create_dir_all(state.join("resolved")).unwrap();
    let o = home.run(&["--overlay", ov, "apply", "--force"]);
    assert!(text(&o).contains("is not a link"), "{}", text(&o));
    // An unmarked directory under generations/ is never pruned.
    std::fs::remove_dir_all(state.join("resolved")).unwrap();
    let foreign = state.join("generations/not-ours");
    std::fs::create_dir_all(&foreign).unwrap();
    for _ in 0..3 {
        skill(&overlay, &format!("s{}", rand_suffix()), "x");
        assert_eq!(home.code(&["--overlay", ov, "apply", "--force"]), 0);
    }
    assert!(foreign.exists());
    let marked = std::fs::read_dir(state.join("generations"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().join(".agent-sync-generation").is_file())
        .count();
    assert_eq!(marked, 2, "live and previous are kept");
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

// ---------- overlay layering ----------

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn skill(dir: &Path, name: &str, body: &str) {
    write(
        &dir.join(format!("skills/{name}/SKILL.md")),
        &format!("---\nname: {name}\ndescription: \"{body}\"\n---\n{body}\n"),
    );
}

/// A temp HOME with the base at `~/git/agent-config` and an overlay at `~/git/private`.
fn layered(name: &str) -> (Home, PathBuf, PathBuf) {
    let home = Home::new(name);
    let base = home.seed_base();
    let overlay = home.path().join("git/private");
    std::fs::create_dir_all(&overlay).unwrap();
    (home, base, overlay)
}

fn resolved(home: &Home) -> PathBuf {
    home.path().join(".local/state/agent-sync/resolved")
}

fn json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn overlay_skill_overrides_the_base_one_and_an_overlay_only_skill_appears() {
    let (home, base, overlay) = layered("ov-skill");
    skill(&overlay, "delegation", "private delegation");
    skill(&overlay, "private-only", "only in the overlay");
    let ov = overlay.to_str().unwrap();
    let o = home.run(&["--overlay", ov, "apply", "--force"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let r = resolved(&home);
    // Every link points into the resolved tree, which points at the winning layer.
    assert_eq!(
        std::fs::read_link(home.path().join(".codex/skills/delegation")).unwrap(),
        r.join("skills/delegation")
    );
    assert_eq!(
        std::fs::read_link(r.join("skills/delegation")).unwrap(),
        overlay.join("skills/delegation")
    );
    // The skill is one unit: the base's references/ does not leak into the overlay's version.
    assert!(!r.join("skills/delegation/references").exists());
    assert!(base.join("skills/delegation/references").exists());
    assert_eq!(
        std::fs::read_link(r.join("skills/rust-craftsman")).unwrap(),
        base.join("skills/rust-craftsman")
    );
    assert!(home.path().join(".codex/skills/private-only").exists());
    assert_eq!(
        std::fs::read_link(home.path().join(".gemini/config/skills")).unwrap(),
        r.join("skills")
    );
    // CLAUDE.md imports resolve through the tree.
    let cm = std::fs::read_to_string(home.path().join(".claude/CLAUDE.md")).unwrap();
    assert!(cm.contains(&format!("@{}", r.join("rules/core.md").display())));
    assert!(std::fs::read_to_string(r.join("rules/core.md")).is_ok());
    // Plugin discovery: the plugin link is the resolved root, with the base's manifest.
    assert_eq!(
        std::fs::read_link(home.path().join(".claude/plugins/nuvek-skills")).unwrap(),
        r
    );
    assert_eq!(home.code(&["--overlay", ov, "check"]), 0);
}

#[test]
fn overlay_mcp_merges_by_key_and_removes_a_base_server() {
    let (home, base, overlay) = layered("ov-mcp");
    write(
        &base.join("sync/mcp.toml"),
        "[servers.keep]\ncommand = \"keep\"\nagents = [\"agy\", \"codex\"]\n\
         [servers.drop]\ncommand = \"drop\"\nagents = [\"agy\", \"codex\"]\n\
         [servers.swap]\ncommand = \"old\"\nagents = [\"agy\"]\n",
    );
    write(
        &overlay.join("sync/mcp.toml"),
        "[remove]\nservers = [\"drop\"]\n\
         [servers.swap]\ncommand = \"new\"\nargs = [\"--x\"]\nagents = [\"agy\"]\n\
         [servers.private]\nurl = \"http://localhost:7000/mcp\"\nagents = [\"agy\", \"codex\"]\n",
    );
    let ov = overlay.to_str().unwrap();
    let o = home.run(&["--overlay", ov, "apply", "--force"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let agy = json(&home.path().join(".gemini/config/mcp_config.json"));
    let servers = agy["mcpServers"].as_object().unwrap();
    let mut names: Vec<_> = servers.keys().map(String::as_str).collect();
    names.sort();
    assert_eq!(names, ["keep", "private", "swap"]);
    assert_eq!(servers["swap"]["command"], "new");
    assert_eq!(servers["private"]["serverUrl"], "http://localhost:7000/mcp");
    let codex = std::fs::read_to_string(home.path().join(".codex/config.toml")).unwrap();
    assert!(codex.contains("[mcp_servers.keep]") && codex.contains("[mcp_servers.private]"));
    assert!(!codex.contains("[mcp_servers.drop]"), "{codex}");
    // The merged file is generated, and names both sources.
    let merged = std::fs::read_to_string(resolved(&home).join("sync/mcp.toml")).unwrap();
    assert!(merged.starts_with(
        "# GENERATED by agent-sync from ~/git/agent-config/sync/mcp.toml merged with ~/git/private/sync/mcp.toml."
    ));
    assert!(!merged.contains("[remove]"));
    assert_eq!(home.code(&["--overlay", ov, "check"]), 0);
}

#[test]
fn an_overlay_that_does_not_render_never_goes_live() {
    let (home, _base, overlay) = layered("ov-invalid");
    let ov = overlay.to_str().unwrap();
    assert_eq!(home.code(&["--overlay", ov, "apply", "--force"]), 0);
    let live = std::fs::read_link(resolved(&home)).unwrap();
    // Both transports on one server: valid TOML, but it cannot be rendered.
    write(
        &overlay.join("sync/mcp.toml"),
        "[servers.bad]\ncommand = \"x\"\nurl = \"http://localhost:1/mcp\"\n",
    );
    let o = home.run(&["--overlay", ov, "apply", "--force"]);
    assert_eq!(o.status.code(), Some(6), "{}", text(&o));
    assert!(text(&o).contains("live tree is unchanged"), "{}", text(&o));
    assert_eq!(std::fs::read_link(resolved(&home)).unwrap(), live);
    assert!(
        !resolved(&home).join("sync/mcp.toml").is_file()
            || !std::fs::read_to_string(resolved(&home).join("sync/mcp.toml"))
                .unwrap()
                .contains("servers.bad")
    );
}

#[test]
fn an_item_that_leaves_both_layers_has_its_stale_link_pruned() {
    let (home, _base, overlay) = layered("ov-prune");
    skill(&overlay, "private-only", "temporary");
    let ov = overlay.to_str().unwrap();
    assert_eq!(home.code(&["--overlay", ov, "apply", "--force"]), 0);
    let link = home.path().join(".codex/skills/private-only");
    assert!(std::fs::symlink_metadata(&link).is_ok());

    std::fs::remove_dir_all(overlay.join("skills/private-only")).unwrap();
    // A base skill dropped by the overlay is pruned the same way.
    write(
        &overlay.join("overlay.toml"),
        "remove = [\"skills/skill-creator\"]\n",
    );
    let o = home.run(&["--overlay", ov, "check"]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(6), "{out}");
    assert!(out.contains("resolved source tree"), "{out}");
    assert!(out.contains("private-only: stale (managed"), "{out}");
    // check never wrote: the link is still there.
    assert!(std::fs::symlink_metadata(&link).is_ok());

    assert_eq!(home.code(&["--overlay", ov, "apply", "--force"]), 0);
    assert!(std::fs::symlink_metadata(&link).is_err());
    assert!(std::fs::symlink_metadata(home.path().join(".codex/skills/skill-creator")).is_err());
    assert!(!resolved(&home).join("skills/skill-creator").exists());
    assert_eq!(home.code(&["--overlay", ov, "check"]), 0);
    // The live generation and the one before it are kept, for a rollback.
    let gens = std::fs::read_dir(home.path().join(".local/state/agent-sync/generations"))
        .unwrap()
        .count();
    assert_eq!(gens, 2);
}

#[test]
fn check_exits_6_on_drift_and_0_when_clean_with_an_overlay() {
    let (home, _base, overlay) = layered("ov-check");
    write(
        &overlay.join("sync/permissions.toml"),
        "[allow]\nshell = [\"make test\"]\n",
    );
    let ov = overlay.to_str().unwrap();
    assert_eq!(home.code(&["--overlay", ov, "check"]), 6);
    assert_eq!(home.code(&["--overlay", ov, "apply", "--force"]), 0);
    assert_eq!(home.code(&["--overlay", ov, "check"]), 0);
    let allow = json(&home.path().join(".claude/settings.json"))["permissions"]["allow"].clone();
    let allow: Vec<_> = allow
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        allow.contains(&"Bash(make test:*)") && allow.contains(&"Bash(ls:*)"),
        "{allow:?}"
    );
    // A projection edited by hand is drift in overlay mode too.
    write(&home.path().join(".codex/AGENTS.md"), "hand edit\n");
    assert_eq!(home.code(&["--overlay", ov, "check"]), 6);
    // Writing commands refuse to target the generated tree.
    assert_eq!(
        home.code(&["--overlay", ov, "promote", "--add", "Bash(x)"]),
        2
    );
}

#[test]
fn an_overlay_agent_names_its_own_source_and_a_renamed_plugin_is_relinked() {
    let (home, base, overlay) = layered("ov-agent");
    let agent = std::fs::read_to_string(base.join("sync/agents/rust-reviewer.md")).unwrap();
    write(
        &overlay.join("sync/agents/rust-reviewer.md"),
        &agent.replace("\n---\n", "\n---\nPrivate preamble.\n"),
    );
    write(
        &overlay.join(".claude-plugin/plugin.json"),
        "{\"name\": \"private-skills\"}\n",
    );
    // First apply without the overlay, then switch to it.
    assert_eq!(home.code(&["apply", "--force"]), 0);
    let ov = overlay.to_str().unwrap();
    let o = home.run(&["--overlay", ov, "apply", "--force"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let rendered =
        std::fs::read_to_string(home.path().join(".claude/agents/rust-reviewer.md")).unwrap();
    assert!(rendered.contains(
        "<!-- GENERATED by agent-sync from ~/git/private/sync/agents/rust-reviewer.md — edit the source, not this file -->"
    ));
    assert!(rendered.contains("Private preamble."));
    let plugins = home.path().join(".claude/plugins");
    assert!(std::fs::symlink_metadata(plugins.join("nuvek-skills")).is_err());
    assert_eq!(
        std::fs::read_link(plugins.join("private-skills")).unwrap(),
        resolved(&home)
    );
    let codex = std::fs::read_to_string(home.path().join(".codex/AGENTS.md")).unwrap();
    assert!(codex
        .starts_with("<!-- GENERATED by agent-sync from ~/git/agent-config + ~/git/private -->\n"));
    // Dropping the overlay again restores the plain layout exactly.
    assert_eq!(home.code(&["apply", "--force"]), 0);
    assert_eq!(home.code(&["check"]), 0);
    let want = std::fs::read_to_string(GOLDEN).unwrap();
    assert!(footprint(home.path()) == want, "plain layout not restored");
}
