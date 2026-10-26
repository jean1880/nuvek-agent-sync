//! Headless loader, component registry, hook execution, and executor selftest for agent-sync.
//!
//! Provides automated verification across Claude Code and Antigravity:
//! - Agent manifests and component registries (rejecting invalid tools like `call_mcp_tool`).
//! - Loader symlink topology and canonical path reachability.
//! - Pre-tool guard hook execution with synthetic safe, chained, and secret payloads.
//! - MCP server registration consistency.
//! - Headless engine CLI sanity checks (`agy agents`, `claude --version`).

use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::model::{Sources, Tool};
use crate::render::validate_agent_frontmatter;
use crate::sync::Paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    Warn,
}

pub struct Probe {
    pub category: &'static str,
    pub name: String,
    pub status: Status,
    pub detail: Option<String>,
}

impl Probe {
    pub fn pass(category: &'static str, name: impl Into<String>) -> Self {
        Self {
            category,
            name: name.into(),
            status: Status::Pass,
            detail: None,
        }
    }

    pub fn warn(
        category: &'static str,
        name: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            category,
            name: name.into(),
            status: Status::Warn,
            detail: Some(detail.into()),
        }
    }

    pub fn fail(
        category: &'static str,
        name: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            category,
            name: name.into(),
            status: Status::Fail,
            detail: Some(detail.into()),
        }
    }
}

pub struct SelftestOptions {
    pub probe_cli: bool,
    pub deep: bool,
    /// A pre-tool guard hook to probe with synthetic payloads; `None` skips those probes.
    pub guard: Option<std::path::PathBuf>,
}

/// Run the full selftest suite across the dual-agent harness.
pub fn run_selftest(p: &Paths, opts: &SelftestOptions) -> Result<i32> {
    let mut probes = Vec::new();

    say!("============================================================");
    say!(" agent-sync selftest — Dual-Agent Harness Verification");
    say!("============================================================");

    let src = p
        .sources()
        .with_context(|| format!("loading sources from {}", p.sync().display()))?;

    // 1. Agent manifests and component validation
    probe_agent_manifests(p, &src, &mut probes)?;

    // 2. Loader symlinks and path reachability
    probe_loader_symlinks(p, &mut probes)?;

    // 3. Hook execution and guard decisions
    match &opts.guard {
        Some(guard) => probe_hooks(guard, &mut probes)?,
        None => probes.push(Probe::warn(
            "PRE-TOOL GUARD HOOK PROBES",
            "guard probes skipped",
            "no guard configured (--guard or $AGENT_SYNC_GUARD)",
        )),
    }

    // 4. MCP registration consistency
    probe_mcp(p, &mut probes)?;

    // 5. CLI tools and engine health
    probe_cli_tools(p, &src, opts, &mut probes)?;

    // Render results
    let mut passed = 0;
    let mut warnings = 0;
    let mut failed = 0;

    let mut current_category = "";
    for probe in &probes {
        if probe.category != current_category {
            current_category = probe.category;
            say!("\n[{current_category}]");
        }
        match probe.status {
            Status::Pass => {
                passed += 1;
                say!("  ✓ {}", probe.name);
            }
            Status::Warn => {
                warnings += 1;
                let detail = probe.detail.as_deref().unwrap_or("");
                say!("  ⚠ {} ({detail})", probe.name);
            }
            Status::Fail => {
                failed += 1;
                let detail = probe.detail.as_deref().unwrap_or("");
                say_err!("  ✗ {} — {detail}", probe.name);
            }
        }
    }

    say!("\n------------------------------------------------------------");
    say!("Results: {passed} passed · {warnings} warnings · {failed} failed");

    if failed > 0 {
        bail!("{failed} selftest probe(s) failed");
    }

    Ok(0)
}

/// Probe agent configurations and ensure tool components resolve correctly.
fn probe_agent_manifests(p: &Paths, src: &Sources, probes: &mut Vec<Probe>) -> Result<()> {
    const VALID_AGY_COMPONENTS: &[&str] = &[
        "view_file",
        "run_command",
        "replace_file_content",
        "write_to_file",
    ];

    for a in &src.agents {
        // Claude agent check
        if a.applies_to(Tool::Claude) {
            let claude_file = p.claude.join(format!("agents/{}.md", a.front.name));
            if !claude_file.exists() {
                probes.push(Probe::fail(
                    "AGENT MANIFESTS & COMPONENTS",
                    format!("claude agent file {}", a.front.name),
                    format!("missing projection at {}", claude_file.display()),
                ));
            } else {
                match fs::read_to_string(&claude_file) {
                    Ok(content) => match crate::model::split_front_matter(&content) {
                        Ok((yaml, body)) => match validate_agent_frontmatter(yaml, Tool::Claude) {
                            Ok(()) => {
                                if body.trim().is_empty() {
                                    probes.push(Probe::warn(
                                        "AGENT MANIFESTS & COMPONENTS",
                                        format!("claude agent {}", a.front.name),
                                        "system prompt body is empty",
                                    ));
                                } else {
                                    probes.push(Probe::pass(
                                        "AGENT MANIFESTS & COMPONENTS",
                                        format!("claude agent {}", a.front.name),
                                    ));
                                }
                            }
                            Err(e) => {
                                probes.push(Probe::fail(
                                    "AGENT MANIFESTS & COMPONENTS",
                                    format!("claude agent {}", a.front.name),
                                    format!("invalid frontmatter: {e:#}"),
                                ));
                            }
                        },
                        Err(e) => {
                            probes.push(Probe::fail(
                                "AGENT MANIFESTS & COMPONENTS",
                                format!("claude agent {}", a.front.name),
                                format!("unparseable frontmatter: {e:#}"),
                            ));
                        }
                    },
                    Err(e) => {
                        probes.push(Probe::fail(
                            "AGENT MANIFESTS & COMPONENTS",
                            format!("claude agent {}", a.front.name),
                            format!("unreadable at {}: {e:#}", claude_file.display()),
                        ));
                    }
                }
            }
        }

        // AGY agent check
        if a.applies_to(Tool::Agy) {
            let agy_file = p.gemini.join(format!("config/agents/{}.md", a.front.name));
            if !agy_file.exists() {
                probes.push(Probe::fail(
                    "AGENT MANIFESTS & COMPONENTS",
                    format!("agy agent file {}", a.front.name),
                    format!("missing projection at {}", agy_file.display()),
                ));
            } else {
                match fs::read_to_string(&agy_file) {
                    Ok(content) => match crate::model::split_front_matter(&content) {
                        Ok((yaml, body)) => {
                            match validate_agent_frontmatter(yaml, Tool::Agy) {
                                Ok(()) => {
                                    // Deep component verification
                                    if let Ok(val) = serde_yaml::from_str::<serde_yaml::Value>(yaml)
                                    {
                                        let mut bad_tool = None;
                                        if let Some(tools_seq) =
                                            val.get("tools").and_then(|v| v.as_sequence())
                                        {
                                            for t in tools_seq {
                                                if let Some(s) = t.as_str() {
                                                    if s == "call_mcp_tool" {
                                                        bad_tool = Some(
                                                            "contains 'call_mcp_tool' (crashes executor at runtime)"
                                                                .to_string(),
                                                        );
                                                        break;
                                                    }
                                                    if !VALID_AGY_COMPONENTS.contains(&s) {
                                                        bad_tool = Some(format!(
                                                            "contains unregistered component '{s}'"
                                                        ));
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                        if let Some(err) = bad_tool {
                                            probes.push(Probe::fail(
                                                "AGENT MANIFESTS & COMPONENTS",
                                                format!("agy agent {}", a.front.name),
                                                err,
                                            ));
                                        } else if body.trim().is_empty() {
                                            probes.push(Probe::warn(
                                                "AGENT MANIFESTS & COMPONENTS",
                                                format!("agy agent {}", a.front.name),
                                                "system prompt body is empty",
                                            ));
                                        } else {
                                            probes.push(Probe::pass(
                                                "AGENT MANIFESTS & COMPONENTS",
                                                format!("agy agent {}", a.front.name),
                                            ));
                                        }
                                    }
                                }
                                Err(e) => {
                                    probes.push(Probe::fail(
                                        "AGENT MANIFESTS & COMPONENTS",
                                        format!("agy agent {}", a.front.name),
                                        format!("invalid frontmatter: {e:#}"),
                                    ));
                                }
                            }
                        }
                        Err(e) => {
                            probes.push(Probe::fail(
                                "AGENT MANIFESTS & COMPONENTS",
                                format!("agy agent {}", a.front.name),
                                format!("unparseable frontmatter: {e:#}"),
                            ));
                        }
                    },
                    Err(e) => {
                        probes.push(Probe::fail(
                            "AGENT MANIFESTS & COMPONENTS",
                            format!("agy agent {}", a.front.name),
                            format!("unreadable at {}: {e:#}", agy_file.display()),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Probe symlink topology and check that discovery links resolve cleanly.
fn probe_loader_symlinks(p: &Paths, probes: &mut Vec<Probe>) -> Result<()> {
    let canonical_skills = p.ac.join("skills");
    if canonical_skills.is_dir() {
        match fs::read_dir(&canonical_skills) {
            Ok(entries) => {
                let count = entries
                    .filter_map(std::result::Result::ok)
                    .filter(|e| e.path().join("SKILL.md").exists())
                    .count();
                probes.push(Probe::pass(
                    "LOADER SYMLINKS & TOPOLOGY",
                    format!("canonical skills directory ({count} skills found)"),
                ));
            }
            Err(e) => {
                probes.push(Probe::fail(
                    "LOADER SYMLINKS & TOPOLOGY",
                    "canonical skills directory",
                    format!("unreadable at {}: {e:#}", canonical_skills.display()),
                ));
            }
        }
    } else {
        probes.push(Probe::fail(
            "LOADER SYMLINKS & TOPOLOGY",
            "canonical skills directory",
            format!("missing directory at {}", canonical_skills.display()),
        ));
    }

    // Claude discovery symlink
    match crate::sync::plugin_name(p) {
        Ok(name) => check_symlink(
            &p.claude.join("skills").join(name),
            &p.ac,
            "claude skills discovery link",
            probes,
        ),
        Err(e) => probes.push(Probe::fail(
            "LOADER SYMLINKS & TOPOLOGY",
            "claude skills discovery link",
            format!("{e:#}"),
        )),
    }

    // AGY skills symlink
    let agy_skills_link = p.gemini.join("config/skills");
    check_symlink(
        &agy_skills_link,
        &canonical_skills,
        "agy config/skills symlink",
        probes,
    );

    // AGY root rules symlink
    let agy_rules_link = p.gemini.join("AGENTS.md");
    check_symlink(
        &agy_rules_link,
        &p.ac.join("rules/core.md"),
        "agy AGENTS.md rules symlink",
        probes,
    );

    // Claude CLAUDE.md rule import
    let claude_md = p.claude.join("CLAUDE.md");
    if claude_md.exists() {
        match fs::read_to_string(&claude_md) {
            Ok(content) => {
                if content.contains("rules/core.md") {
                    probes.push(Probe::pass(
                        "LOADER SYMLINKS & TOPOLOGY",
                        "claude root CLAUDE.md imports rules/core.md",
                    ));
                } else {
                    probes.push(Probe::warn(
                        "LOADER SYMLINKS & TOPOLOGY",
                        "claude root CLAUDE.md",
                        "does not reference rules/core.md",
                    ));
                }
            }
            Err(e) => {
                probes.push(Probe::fail(
                    "LOADER SYMLINKS & TOPOLOGY",
                    "claude root CLAUDE.md",
                    format!("unreadable at {}: {e:#}", claude_md.display()),
                ));
            }
        }
    } else {
        probes.push(Probe::fail(
            "LOADER SYMLINKS & TOPOLOGY",
            "claude root CLAUDE.md",
            format!("missing at {}", claude_md.display()),
        ));
    }

    // Check for dangling symlinks in key directories
    check_dangling(&p.gemini.join("config/skills"), "agy skills", probes);
    check_dangling(&p.claude.join("skills"), "claude skills", probes);

    Ok(())
}

fn check_symlink(link: &Path, expected_target: &Path, name: &str, probes: &mut Vec<Probe>) {
    match fs::symlink_metadata(link) {
        Ok(meta) => {
            if !meta.file_type().is_symlink() {
                probes.push(Probe::fail(
                    "LOADER SYMLINKS & TOPOLOGY",
                    name,
                    format!("path {} is not a symlink", link.display()),
                ));
                return;
            }
            match fs::canonicalize(link) {
                Ok(resolved) => {
                    if let Ok(expected_resolved) = fs::canonicalize(expected_target) {
                        if resolved == expected_resolved {
                            probes.push(Probe::pass(
                                "LOADER SYMLINKS & TOPOLOGY",
                                format!("{name} -> {}", expected_target.display()),
                            ));
                        } else {
                            probes.push(Probe::fail(
                                "LOADER SYMLINKS & TOPOLOGY",
                                name,
                                format!(
                                    "resolves to {} instead of expected {}",
                                    resolved.display(),
                                    expected_resolved.display()
                                ),
                            ));
                        }
                    } else {
                        probes.push(Probe::fail(
                            "LOADER SYMLINKS & TOPOLOGY",
                            name,
                            format!(
                                "expected target {} does not exist",
                                expected_target.display()
                            ),
                        ));
                    }
                }
                Err(e) => {
                    probes.push(Probe::fail(
                        "LOADER SYMLINKS & TOPOLOGY",
                        name,
                        format!("broken symlink: {e:#}"),
                    ));
                }
            }
        }
        Err(e) => {
            probes.push(Probe::fail(
                "LOADER SYMLINKS & TOPOLOGY",
                name,
                format!("missing path {}: {e:#}", link.display()),
            ));
        }
    }
}

fn check_dangling(dir: &Path, label: &str, probes: &mut Vec<Probe>) {
    if !dir.exists() {
        return;
    }
    match fs::read_dir(dir) {
        Ok(entries) => {
            let mut dangling_count = 0;
            for entry in entries {
                let Ok(entry) = entry else { continue };
                let p = entry.path();
                if let Ok(meta) = fs::symlink_metadata(&p) {
                    if meta.file_type().is_symlink() && fs::canonicalize(&p).is_err() {
                        dangling_count += 1;
                    }
                }
            }
            if dangling_count == 0 {
                probes.push(Probe::pass(
                    "LOADER SYMLINKS & TOPOLOGY",
                    format!("no dangling symlinks under {label}"),
                ));
            } else {
                probes.push(Probe::fail(
                    "LOADER SYMLINKS & TOPOLOGY",
                    format!("{label} symlink integrity"),
                    format!(
                        "{dangling_count} dangling symlink(s) found under {}",
                        dir.display()
                    ),
                ));
            }
        }
        Err(e) => {
            probes.push(Probe::fail(
                "LOADER SYMLINKS & TOPOLOGY",
                format!("{label} directory read"),
                format!("unreadable at {}: {e:#}", dir.display()),
            ));
        }
    }
}

/// Dynamically probe a pre-tool guard hook with synthetic payloads. The guard contract: read the
/// tool call on stdin, exit 0 for an allowed call, and print a deny decision naming the rule
/// (`NO-CHAINING` for a chained shell command, `SECRETS` for reading key material).
fn probe_hooks(guard_script: &Path, probes: &mut Vec<Probe>) -> Result<()> {
    if !guard_script.exists() {
        probes.push(Probe::fail(
            "PRE-TOOL GUARD HOOK PROBES",
            "guard script existence",
            format!("missing {}", guard_script.display()),
        ));
        return Ok(());
    }

    // Syntax check
    match Command::new("bash").arg("-n").arg(guard_script).output() {
        Ok(syntax) => {
            if syntax.status.success() {
                probes.push(Probe::pass(
                    "PRE-TOOL GUARD HOOK PROBES",
                    "syntax check (bash -n <guard>)",
                ));
            } else {
                probes.push(Probe::fail(
                    "PRE-TOOL GUARD HOOK PROBES",
                    "syntax check (bash -n <guard>)",
                    String::from_utf8_lossy(&syntax.stderr).to_string(),
                ));
                return Ok(());
            }
        }
        Err(e) => {
            probes.push(Probe::fail(
                "PRE-TOOL GUARD HOOK PROBES",
                "syntax check (bash -n <guard>)",
                format!("failed to spawn bash: {e:#}"),
            ));
            return Ok(());
        }
    }

    // Dynamic test: AGY safe command
    let agy_safe = r#"{"toolCall": {"name": "run_command", "args": {"CommandLine": "ls"}}}"#;
    match run_guard_probe(guard_script, agy_safe, "deny") {
        Ok((code, stdout)) => {
            if code == 0 && !stdout.contains(r#""decision":"deny""#) {
                probes.push(Probe::pass(
                    "PRE-TOOL GUARD HOOK PROBES",
                    "agy safe command allowed (ls)",
                ));
            } else {
                probes.push(Probe::fail(
                    "PRE-TOOL GUARD HOOK PROBES",
                    "agy safe command allowed (ls)",
                    format!("unexpectedly blocked: code={code}, stdout={stdout}"),
                ));
            }
        }
        Err(e) => {
            probes.push(Probe::fail(
                "PRE-TOOL GUARD HOOK PROBES",
                "agy safe command allowed (ls)",
                format!("failed to run: {e:#}"),
            ));
        }
    }

    // Dynamic test: AGY chained command
    let agy_chained =
        r#"{"toolCall": {"name": "run_command", "args": {"CommandLine": "ls && whoami"}}}"#;
    match run_guard_probe(guard_script, agy_chained, "deny") {
        Ok((_, stdout)) => {
            if stdout.contains(r#""decision":"deny""#) && stdout.contains("NO-CHAINING") {
                probes.push(Probe::pass(
                    "PRE-TOOL GUARD HOOK PROBES",
                    "agy chained command blocked (ls && whoami)",
                ));
            } else {
                probes.push(Probe::fail(
                    "PRE-TOOL GUARD HOOK PROBES",
                    "agy chained command blocked (ls && whoami)",
                    format!("failed to deny chained command: stdout={stdout}"),
                ));
            }
        }
        Err(e) => {
            probes.push(Probe::fail(
                "PRE-TOOL GUARD HOOK PROBES",
                "agy chained command blocked (ls && whoami)",
                format!("failed to run: {e:#}"),
            ));
        }
    }

    // Dynamic test: AGY secret inspection
    let agy_secret = r#"{"toolCall": {"name": "run_command", "args": {"CommandLine": "cat ~/.ssh/id_ed25519"}}}"#;
    match run_guard_probe(guard_script, agy_secret, "deny") {
        Ok((_, stdout)) => {
            if stdout.contains(r#""decision":"deny""#) && stdout.contains("SECRETS") {
                probes.push(Probe::pass(
                    "PRE-TOOL GUARD HOOK PROBES",
                    "agy secret command blocked (cat ~/.ssh/id_ed25519)",
                ));
            } else {
                probes.push(Probe::fail(
                    "PRE-TOOL GUARD HOOK PROBES",
                    "agy secret command blocked (cat ~/.ssh/id_ed25519)",
                    format!("failed to deny secret inspection: stdout={stdout}"),
                ));
            }
        }
        Err(e) => {
            probes.push(Probe::fail(
                "PRE-TOOL GUARD HOOK PROBES",
                "agy secret command blocked (cat ~/.ssh/id_ed25519)",
                format!("failed to run: {e:#}"),
            ));
        }
    }

    // Dynamic test: Claude format safe command
    let claude_safe = r#"{"tool_name": "Bash", "tool_input": {"command": "git status"}}"#;
    match run_guard_probe(guard_script, claude_safe, "deny") {
        Ok((code, stdout)) => {
            if code == 0 && !stdout.contains(r#""permissionDecision":"deny""#) {
                probes.push(Probe::pass(
                    "PRE-TOOL GUARD HOOK PROBES",
                    "claude format safe command allowed (git status)",
                ));
            } else {
                probes.push(Probe::fail(
                    "PRE-TOOL GUARD HOOK PROBES",
                    "claude format safe command allowed (git status)",
                    format!("unexpectedly blocked: code={code}, stdout={stdout}"),
                ));
            }
        }
        Err(e) => {
            probes.push(Probe::fail(
                "PRE-TOOL GUARD HOOK PROBES",
                "claude format safe command allowed (git status)",
                format!("failed to run: {e:#}"),
            ));
        }
    }

    // Dynamic test: Claude format chained command
    let claude_chained =
        r#"{"tool_name": "Bash", "tool_input": {"command": "git status; whoami"}}"#;
    match run_guard_probe(guard_script, claude_chained, "deny") {
        Ok((_, stdout)) => {
            if stdout.contains(r#""permissionDecision":"deny""#) && stdout.contains("NO-CHAINING") {
                probes.push(Probe::pass(
                    "PRE-TOOL GUARD HOOK PROBES",
                    "claude format chained command blocked (git status; whoami)",
                ));
            } else {
                probes.push(Probe::fail(
                    "PRE-TOOL GUARD HOOK PROBES",
                    "claude format chained command blocked (git status; whoami)",
                    format!("failed to deny chained command: stdout={stdout}"),
                ));
            }
        }
        Err(e) => {
            probes.push(Probe::fail(
                "PRE-TOOL GUARD HOOK PROBES",
                "claude format chained command blocked (git status; whoami)",
                format!("failed to run: {e:#}"),
            ));
        }
    }

    Ok(())
}

fn run_guard_probe(guard_script: &Path, payload: &str, guard_mode: &str) -> Result<(i32, String)> {
    let mut child = Command::new("bash")
        .arg(guard_script)
        .env("GUARD_MODE", guard_mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning guard script {}", guard_script.display()))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(payload.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok((code, stdout))
}

/// Verify MCP configuration files in both tools.
fn probe_mcp(p: &Paths, probes: &mut Vec<Probe>) -> Result<()> {
    // Claude JSON
    if p.claude_json.exists() {
        match fs::read_to_string(&p.claude_json) {
            Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(v) => {
                    if let Some(servers) = v.get("mcpServers").and_then(|v| v.as_object()) {
                        probes.push(Probe::pass(
                            "MCP REGISTRATION CONSISTENCY",
                            format!(
                                "claude: ~/.claude.json contains {} server(s)",
                                servers.len()
                            ),
                        ));
                    } else {
                        probes.push(Probe::fail(
                            "MCP REGISTRATION CONSISTENCY",
                            "claude ~/.claude.json",
                            "missing or invalid 'mcpServers' object",
                        ));
                    }
                }
                Err(e) => {
                    probes.push(Probe::fail(
                        "MCP REGISTRATION CONSISTENCY",
                        "claude ~/.claude.json",
                        format!("invalid JSON: {e:#}"),
                    ));
                }
            },
            Err(e) => {
                probes.push(Probe::fail(
                    "MCP REGISTRATION CONSISTENCY",
                    "claude ~/.claude.json",
                    format!("unreadable at {}: {e:#}", p.claude_json.display()),
                ));
            }
        }
    } else {
        probes.push(Probe::fail(
            "MCP REGISTRATION CONSISTENCY",
            "claude ~/.claude.json",
            "file does not exist",
        ));
    }

    // AGY MCP Config
    let agy_mcp = p.gemini.join("config/mcp_config.json");
    if agy_mcp.exists() {
        match fs::read_to_string(&agy_mcp) {
            Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(v) => {
                    if let Some(servers) = v.get("mcpServers").and_then(|v| v.as_object()) {
                        probes.push(Probe::pass(
                            "MCP REGISTRATION CONSISTENCY",
                            format!("agy: mcp_config.json contains {} server(s)", servers.len()),
                        ));
                    } else {
                        probes.push(Probe::fail(
                            "MCP REGISTRATION CONSISTENCY",
                            "agy mcp_config.json",
                            "missing or invalid 'mcpServers' object",
                        ));
                    }
                }
                Err(e) => {
                    probes.push(Probe::fail(
                        "MCP REGISTRATION CONSISTENCY",
                        "agy mcp_config.json",
                        format!("invalid JSON: {e:#}"),
                    ));
                }
            },
            Err(e) => {
                probes.push(Probe::fail(
                    "MCP REGISTRATION CONSISTENCY",
                    "agy mcp_config.json",
                    format!("unreadable at {}: {e:#}", agy_mcp.display()),
                ));
            }
        }
    } else {
        probes.push(Probe::fail(
            "MCP REGISTRATION CONSISTENCY",
            "agy mcp_config.json",
            "file does not exist",
        ));
    }

    Ok(())
}

/// Probe CLI commands and verify agent visibility.
fn probe_cli_tools(
    _p: &Paths,
    src: &Sources,
    opts: &SelftestOptions,
    probes: &mut Vec<Probe>,
) -> Result<()> {
    let expected_agy_agents: HashSet<String> = src
        .agents
        .iter()
        .filter(|a| a.applies_to(Tool::Agy))
        .map(|a| a.front.name.clone())
        .collect();

    // Verify agy binary and agent list
    let agy_cmd = Command::new("agy").arg("agents").output();
    match agy_cmd {
        Ok(out) => {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let found_agents: HashSet<String> = stdout
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && *l != "Available agents:")
                    .map(String::from)
                    .collect();

                let missing: Vec<&str> = expected_agy_agents
                    .difference(&found_agents)
                    .map(String::as_str)
                    .collect();

                if missing.is_empty() {
                    probes.push(Probe::pass(
                        "ENGINE & CLI HEALTH",
                        format!(
                            "agy CLI lists all {} declared agent(s)",
                            expected_agy_agents.len()
                        ),
                    ));
                } else {
                    probes.push(Probe::fail(
                        "ENGINE & CLI HEALTH",
                        "agy agents listing",
                        format!("missing expected agent(s): {}", missing.join(", ")),
                    ));
                }
            } else {
                probes.push(Probe::fail(
                    "ENGINE & CLI HEALTH",
                    "agy agents command",
                    format!("exited with code {:?}", out.status.code()),
                ));
            }
        }
        Err(e) => {
            probes.push(Probe::fail(
                "ENGINE & CLI HEALTH",
                "agy binary",
                format!("failed to execute 'agy agents': {e:#}"),
            ));
        }
    }

    // Verify claude binary
    let claude_cmd = Command::new("claude").arg("--version").output();
    match claude_cmd {
        Ok(out) => {
            if out.status.success() {
                let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
                probes.push(Probe::pass(
                    "ENGINE & CLI HEALTH",
                    format!("claude CLI available ({ver})"),
                ));
            } else {
                probes.push(Probe::fail(
                    "ENGINE & CLI HEALTH",
                    "claude --version",
                    format!("exited with code {:?}", out.status.code()),
                ));
            }
        }
        Err(e) => {
            probes.push(Probe::fail(
                "ENGINE & CLI HEALTH",
                "claude binary",
                format!("failed to execute 'claude --version': {e:#}"),
            ));
        }
    }

    // Deep doctor probe if requested
    if opts.deep || opts.probe_cli {
        let doctor = Command::new("claude").arg("doctor").output();
        match doctor {
            Ok(out) => {
                if out.status.success() {
                    probes.push(Probe::pass(
                        "ENGINE & CLI HEALTH",
                        "claude doctor passed cleanly",
                    ));
                } else {
                    probes.push(Probe::warn(
                        "ENGINE & CLI HEALTH",
                        "claude doctor",
                        format!("exited with code {:?}", out.status.code()),
                    ));
                }
            }
            Err(e) => {
                probes.push(Probe::warn(
                    "ENGINE & CLI HEALTH",
                    "claude doctor",
                    format!("failed to execute: {e:#}"),
                ));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_helpers_construct_expected_statuses() {
        let p_pass = Probe::pass("TEST", "pass test");
        assert_eq!(p_pass.status, Status::Pass);
        assert!(p_pass.detail.is_none());

        let p_warn = Probe::warn("TEST", "warn test", "warning detail");
        assert_eq!(p_warn.status, Status::Warn);
        assert_eq!(p_warn.detail.as_deref(), Some("warning detail"));

        let p_fail = Probe::fail("TEST", "fail test", "failure detail");
        assert_eq!(p_fail.status, Status::Fail);
        assert_eq!(p_fail.detail.as_deref(), Some("failure detail"));
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("agent-sync-selftest-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn check_dangling_identifies_broken_symlinks() -> Result<()> {
        let dir = tmp("dangling");
        let target = dir.join("real.txt");
        fs::write(&target, "content")?;

        let valid_link = dir.join("valid_link");
        std::os::unix::fs::symlink(&target, &valid_link)?;

        let mut probes = Vec::new();
        check_dangling(&dir, "test dir", &mut probes);
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].status, Status::Pass);

        // Add a dangling symlink
        let non_existent = dir.join("deleted.txt");
        let broken_link = dir.join("broken_link");
        std::os::unix::fs::symlink(&non_existent, &broken_link)?;

        let mut probes2 = Vec::new();
        check_dangling(&dir, "test dir", &mut probes2);
        assert_eq!(probes2.len(), 1);
        assert_eq!(probes2[0].status, Status::Fail);
        assert!(probes2[0].detail.as_deref().unwrap().contains("1 dangling"));

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn validate_agent_catches_call_mcp_tool_in_agy() {
        let bad_yaml =
            "name: test-agent\ndescription: test\nmodel: inherit\ntools:\n  - call_mcp_tool\n";
        let res = validate_agent_frontmatter(bad_yaml, Tool::Agy);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("call_mcp_tool"));

        let good_yaml = "name: test-agent\ndescription: test\nmodel: inherit\ntools:\n  - view_file\nenable_mcp_tools: true\n";
        assert!(validate_agent_frontmatter(good_yaml, Tool::Agy).is_ok());
    }
}
