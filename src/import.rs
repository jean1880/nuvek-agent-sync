//! `agent-sync import`: seed the neutral sources from the current Claude configuration (the
//! richer of the two trees). Re-runnable with `--force`; never reads settings.local.json (those
//! are unpromoted approvals, handled by `promote`).

use crate::model::{
    AgentFront, Hook, HookFile, HookSide, MapFile, McpFile, McpServer, PermFile, PermSet, Tool,
};
use crate::render::parse_perm;
use crate::sync::{atomic_write, Paths};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;

pub const DEFAULT_MAP: &str = r#"# THE translation table between the neutral sources and each tool. Edit here; nothing is
# hard-coded in agent-sync.

# Public env names whose ${NAME} references are expanded when rendering (IPs stay out of git).
# ONLY these are expanded — a secret reference such as ${MY_API_TOKEN} is always left verbatim.
placeholders = []

[tiers.claude]
fast = "haiku"
standard = "sonnet"
deep = "opus"

[tiers.agy]
fast = "flash"
standard = "pro"
deep = "pro"

[tools.claude]
shell = "Bash"
read = "Read"
grep = "Grep"
glob = "Glob"
edit = "Edit"
write = "Write"

[tools.agy]
shell = "run_command"
read = "view_file"
grep = "run_command"
glob = "run_command"
edit = "replace_file_content"
write = "write_to_file"
mcp = "call_mcp_tool"

# Documentation only — surfaces deliberately NOT projected to the other tool.
[not_projected]
claude = []
agy = []
"#;

fn write_new(path: &std::path::Path, text: &str, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!("{} exists (pass --force to overwrite)", path.display());
    }
    atomic_write(path, text)
}

/// Lenient `key: value` reader for the Claude agent files, whose unquoted descriptions are not
/// always strict YAML. Only top-level scalar keys are read.
fn lenient_front(yaml: &str) -> BTreeMap<String, String> {
    yaml.lines()
        .filter(|l| !l.starts_with(' ') && !l.starts_with('\t'))
        .filter_map(|l| l.split_once(": ").or_else(|| l.split_once(':')))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

fn render_front(fr: &AgentFront) -> Result<String> {
    // serde_yaml emits strict YAML (quotes where needed).
    Ok(serde_yaml::to_string(fr)?)
}

pub fn perm_toml(pf: &PermFile) -> Result<String> {
    Ok(format!(
        "# Neutral permissions, rendered to Claude settings.json and AGY settings.json by agent-sync.\n\
         # Machine-rewritten by `agent-sync promote`: comments here are not preserved.\n\
         # shell = Bash(X:*) / command(X) · shell_glob = Bash(X*) / command(X) · shell_exact = Bash(X) / command(X)\n\
         # read = Read(//abs) / read_file(/abs) · mcp = mcp__s__t / mcp(s/t) · claude/agy = raw, one tool only\n\n{}",
        toml::to_string_pretty(pf)?
    ))
}

fn import_perms(set: &[String]) -> PermSet {
    let mut ps = PermSet::default();
    for e in set {
        let (slot, v) = parse_perm(e, Tool::Claude);
        match slot {
            "shell" => ps.shell.push(v),
            "shell_glob" => ps.shell_glob.push(v),
            "shell_exact" => ps.shell_exact.push(v),
            "read" => ps.read.push(v),
            "mcp" => ps.mcp.push(v),
            _ => ps.claude.push(v),
        }
    }
    ps
}

/// Replace literal values of the allow-listed placeholder vars with `${NAME}`.
fn mask(s: &str, map: &MapFile) -> String {
    let mut out = s.to_string();
    for name in &map.placeholders {
        if let Some(v) = nuvek_core::env::env_var(name).filter(|v| v.len() >= 4) {
            out = out.replace(&v, &format!("${{{name}}}"));
        }
    }
    out
}

fn hook_name(command: &str, taken: &mut Vec<String>) -> String {
    let base = command
        .split_whitespace()
        .rfind(|t| t.contains('/'))
        .and_then(|t| t.trim_matches(|c| c == '\'' || c == '"').rsplit('/').next())
        .map(|t| t.trim_end_matches(".sh").to_string())
        .unwrap_or_else(|| "hook".into());
    let mode = command
        .split_whitespace()
        .find_map(|t| t.strip_prefix("GUARD_MODE="))
        .map(|m| format!("-{m}"))
        .unwrap_or_default();
    let mut name = format!("{base}{mode}");
    let mut i = 2;
    while taken.contains(&name) {
        name = format!("{base}{mode}-{i}");
        i += 1;
    }
    taken.push(name.clone());
    name
}

/// The agent's front-matter `name`, which becomes a file name: one plain component equal to the
/// file's own stem, so `../../victim` or an absolute name can never pick the write location.
fn checked_name(path: &std::path::Path, kv: &BTreeMap<String, String>) -> Result<String> {
    let name = kv
        .get("name")
        .with_context(|| format!("{}: no 'name'", path.display()))?;
    if !crate::model::valid_name(name) {
        bail!(
            "{}: name '{name}' must be [A-Za-z0-9._-]+ and not start with a dot",
            path.display()
        );
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string());
    if stem.as_deref() != Some(name.as_str()) {
        bail!("{}: name '{name}' must match the file name", path.display());
    }
    Ok(name.clone())
}

pub fn run(p: &Paths, force: bool) -> Result<()> {
    let sync = p.sync();
    let mut agents = std::fs::read_dir(p.claude.join("agents"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|x| x.extension().is_some_and(|e| e == "md"))
        .collect::<Vec<_>>();
    agents.sort();
    // Every name is checked before anything is written.
    for path in &agents {
        let text = std::fs::read_to_string(path)?;
        let (yaml, _) = crate::model::split_front_matter(&text)
            .with_context(|| format!("{}", path.display()))?;
        checked_name(path, &lenient_front(yaml))?;
    }
    std::fs::create_dir_all(sync.join("agents"))?;
    write_new(&sync.join("map.toml"), DEFAULT_MAP, force)?;
    let map: MapFile = toml::from_str(DEFAULT_MAP)?;

    // Agents.
    for path in agents {
        let text = std::fs::read_to_string(&path)?;
        let (yaml, body) = crate::model::split_front_matter(&text)
            .with_context(|| format!("{}", path.display()))?;
        let kv = lenient_front(yaml);
        let get = |k: &str| {
            kv.get(k)
                .cloned()
                .with_context(|| format!("{}: no '{k}'", path.display()))
        };
        let model = get("model")?;
        let tier = map
            .tier_of(Tool::Claude, &model)
            .with_context(|| format!("{}: model '{model}' has no tier", path.display()))?;
        let mut tools = Vec::new();
        for t in kv
            .get("tools")
            .map(|s| {
                s.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
        {
            if let Some(server) = t.strip_prefix("mcp__") {
                tools.push(format!("mcp:{server}"));
            } else {
                tools.push(
                    map.neutral_tool(Tool::Claude, t).with_context(|| {
                        format!("{}: tool '{t}' not in map.toml", path.display())
                    })?,
                );
            }
        }
        let mut claude = serde_yaml::Mapping::new();
        for (k, v) in &kv {
            if !["name", "description", "tools", "model"].contains(&k.as_str()) {
                let val: serde_yaml::Value =
                    serde_yaml::from_str(v).unwrap_or(serde_yaml::Value::String(v.clone()));
                claude.insert(serde_yaml::Value::String(k.clone()), val);
            }
        }
        let fr = AgentFront {
            name: checked_name(&path, &kv)?,
            description: get("description")?,
            tier,
            tools,
            agents: vec!["claude".into(), "agy".into()],
            claude,
            agy: serde_yaml::Mapping::new(),
        };
        let out = format!("---\n{}---\n{}", render_front(&fr)?, body);
        write_new(
            &sync.join("agents").join(format!("{}.md", fr.name)),
            &out,
            force,
        )?;
    }

    // MCP servers.
    let cj: Value = serde_json::from_str(&std::fs::read_to_string(&p.claude_json)?)?;
    let mut mcp = McpFile::default();
    if let Some(servers) = cj.get("mcpServers").and_then(Value::as_object) {
        for (name, v) in servers {
            let s = |k: &str| v.get(k).and_then(Value::as_str).map(|x| mask(x, &map));
            // An HTTP server is `{"type":"http","url"}`: carry the url, not the type.
            let url = s("url");
            let srv = McpServer {
                kind: if url.is_some() { None } else { s("type") },
                command: s("command"),
                url,
                args: v
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(|x| mask(x, &map))
                            .collect()
                    })
                    .unwrap_or_default(),
                env: v
                    .get("env")
                    .and_then(Value::as_object)
                    .map(|o| {
                        o.iter()
                            .filter_map(|(k, x)| x.as_str().map(|x| (k.clone(), mask(x, &map))))
                            .collect()
                    })
                    .unwrap_or_default(),
                agents: vec!["claude".into(), "agy".into()],
            };
            mcp.servers.insert(name.clone(), srv);
        }
    }
    write_new(&sync.join("mcp.toml"), &format!("# MCP servers for both tools (agents = which tools get each). No secret values: reference them\n# as $VAR inside the bash -c wrapper. Only map.toml placeholders are expanded.\n\n{}", toml::to_string_pretty(&mcp)?), force)?;

    // Permissions + hooks from settings.json.
    let cs: Value =
        serde_json::from_str(&std::fs::read_to_string(p.claude.join("settings.json"))?)?;
    let list = |ptr: &str| -> Vec<String> {
        cs.pointer(ptr)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    let pf = PermFile {
        allow: import_perms(&list("/permissions/allow")),
        deny: import_perms(&list("/permissions/deny")),
    };
    write_new(&sync.join("permissions.toml"), &perm_toml(&pf)?, force)?;

    let mut hf = HookFile::default();
    let mut taken = Vec::new();
    if let Some(events) = cs.get("hooks").and_then(Value::as_object) {
        for (event, entries) in events {
            for entry in entries.as_array().into_iter().flatten() {
                let matcher = entry
                    .get("matcher")
                    .and_then(Value::as_str)
                    .map(String::from);
                for h in entry
                    .get("hooks")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let command = h
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    hf.hooks.push(Hook {
                        name: hook_name(&command, &mut taken),
                        event: event.clone(),
                        command,
                        claude: Some(HookSide {
                            matcher: matcher.clone(),
                            timeout: h.get("timeout").and_then(Value::as_u64),
                            ..Default::default()
                        }),
                        agy: None,
                        codex: None,
                    });
                }
            }
        }
    }
    write_new(&sync.join("hooks.toml"), &format!("# Hooks for both tools. A side (claude/agy) that is absent is not projected to that tool.\n# timeout is in SECONDS for both tools.\n\n{}", toml::to_string_pretty(&hf)?), force)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_names_are_derived_and_unique() {
        let mut t = Vec::new();
        assert_eq!(
            hook_name("GUARD_MODE=deny bash /x/example-guard.sh", &mut t),
            "example-guard-deny"
        );
        assert_eq!(
            hook_name("GUARD_MODE=warn bash /x/example-guard.sh", &mut t),
            "example-guard-warn"
        );
        assert_eq!(
            hook_name("bash /x/example-on-edit.sh", &mut t),
            "example-on-edit"
        );
        assert_eq!(
            hook_name("bash /x/example-on-edit.sh", &mut t),
            "example-on-edit-2"
        );
        assert_eq!(
            hook_name("bash -c 'source $HOME/.e && /r/example-hook'", &mut t),
            "example-hook"
        );
    }

    #[test]
    fn a_traversing_or_absolute_agent_name_is_refused_before_any_write() {
        for (file, name) in [
            ("victim", "../../victim"),
            ("abs", "/tmp/abs"),
            ("ok", "other"),
        ] {
            let home = std::env::temp_dir().join(crate::fsx::unique("agent-sync-import"));
            let p = Paths::from_home(&home, None);
            std::fs::create_dir_all(p.claude.join("agents")).unwrap();
            std::fs::write(
                p.claude.join(format!("agents/{file}.md")),
                format!("---\nname: {name}\ndescription: d\nmodel: sonnet\n---\nbody\n"),
            )
            .unwrap();
            p.register_write_roots();
            let err = run(&p, false).unwrap_err().to_string();
            assert!(err.contains("name"), "{name}: {err}");
            assert!(!p.sync().exists(), "{name}: something was written");
            assert!(!home.join("victim.md").exists());
            let _ = std::fs::remove_dir_all(&home);
        }
    }

    #[test]
    fn default_map_parses() {
        let m: MapFile = toml::from_str(DEFAULT_MAP).unwrap();
        assert_eq!(m.model(Tool::Agy, "standard").unwrap(), "pro");
        assert_eq!(
            m.neutral_tool(Tool::Claude, "Bash").as_deref(),
            Some("shell")
        );
    }
}
