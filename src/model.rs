//! The neutral source formats in the agent-config checkout's `sync/` directory.
//!
//! Every file here is authored once and rendered per tool by `render`. Nothing in these structs
//! may hold a secret VALUE: MCP commands reference secrets as `$VAR` inside their `bash -c`
//! wrapper, and the only `${VAR}` expansion agent-sync performs is for the public names listed in
//! `map.toml` `placeholders`.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The two projection targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Claude,
    Agy,
}

impl Tool {
    pub fn key(self) -> &'static str {
        match self {
            Tool::Claude => "claude",
            Tool::Agy => "agy",
        }
    }
}

fn both() -> Vec<String> {
    vec!["claude".into(), "agy".into()]
}

/// `map.toml`: the ONE translation table. No mapping is hard-coded in the renderers.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MapFile {
    /// tool → (tier → model), e.g. `claude.standard = "sonnet"`.
    pub tiers: BTreeMap<String, BTreeMap<String, String>>,
    /// tool → (neutral tool name → tool-specific name). `mcp` is the AGY name for every
    /// `mcp:<server>` grant; Claude renders those as `mcp__<server>`.
    pub tools: BTreeMap<String, BTreeMap<String, String>>,
    /// Public env names whose `${NAME}` references are expanded at render time (and substituted
    /// back in by `import`), so IPs stay out of git. Anything NOT listed is left verbatim —
    /// that is what keeps `${MY_API_TOKEN}`-style secret references from ever being inlined.
    #[serde(default)]
    pub placeholders: Vec<String>,
    /// Documentation only: surfaces deliberately not projected, per tool.
    #[serde(default)]
    pub not_projected: BTreeMap<String, Vec<String>>,
}

impl MapFile {
    pub fn model(&self, tool: Tool, tier: &str) -> Result<String> {
        self.tiers
            .get(tool.key())
            .and_then(|m| m.get(tier))
            .cloned()
            .with_context(|| format!("map.toml has no tier '{tier}' for {}", tool.key()))
    }

    pub fn tool_name(&self, tool: Tool, neutral: &str) -> Result<String> {
        self.tools
            .get(tool.key())
            .and_then(|m| m.get(neutral))
            .cloned()
            .with_context(|| format!("map.toml has no tool '{neutral}' for {}", tool.key()))
    }

    /// Reverse lookup for `import`: tool-specific model → tier.
    pub fn tier_of(&self, tool: Tool, model: &str) -> Option<String> {
        self.tiers
            .get(tool.key())?
            .iter()
            .find(|(_, m)| m.as_str() == model)
            .map(|(t, _)| t.clone())
    }

    /// Reverse lookup for `import`: tool-specific tool name → neutral name (first match).
    pub fn neutral_tool(&self, tool: Tool, name: &str) -> Option<String> {
        self.tools
            .get(tool.key())?
            .iter()
            .find(|(k, v)| v.as_str() == name && k.as_str() != "mcp")
            .map(|(k, _)| k.clone())
    }
}

/// Front matter of a neutral agent (`sync/agents/<name>.md`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentFront {
    pub name: String,
    pub description: String,
    pub tier: String,
    #[serde(default)]
    pub tools: Vec<String>,
    /// Which tools get this agent (default: both).
    #[serde(default = "both")]
    pub agents: Vec<String>,
    /// Extra front-matter keys passed through to one tool only (e.g. Claude `maxTurns`, `color`).
    #[serde(default, skip_serializing_if = "serde_yaml::Mapping::is_empty")]
    pub claude: serde_yaml::Mapping,
    #[serde(default, skip_serializing_if = "serde_yaml::Mapping::is_empty")]
    pub agy: serde_yaml::Mapping,
}

#[derive(Debug, Clone)]
pub struct Agent {
    pub front: AgentFront,
    pub body: String,
    /// The source file as the generated header names it (`~/…/sync/agents/<name>.md`). Empty for
    /// an agent parsed from text alone.
    pub origin: String,
}

/// An agent name becomes a file name in every tool's agents directory, so it must be exactly
/// one plain path component: `[A-Za-z0-9._-]+`, not starting with a dot.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Split `---\n<yaml>\n---\n<body>`. An EMPTY block (`---\n---\n`) is rejected as unclosed; that is
/// unreachable for agents and skills, which always need `name` + `description`.
pub fn split_front_matter(text: &str) -> Result<(&str, &str)> {
    let rest = text
        .strip_prefix("---\n")
        .context("missing opening '---' front-matter fence")?;
    let end = rest
        .find("\n---\n")
        .or_else(|| rest.strip_suffix("\n---").map(|r| r.len()))
        .context("missing closing '---' front-matter fence")?;
    let yaml = &rest[..end];
    let body = rest[end..]
        .strip_prefix("\n---\n")
        .or_else(|| rest[end..].strip_prefix("\n---"))
        .unwrap_or("");
    Ok((yaml, body))
}

impl Agent {
    pub fn parse(text: &str) -> Result<Agent> {
        let (yaml, body) = split_front_matter(text)?;
        let front: AgentFront =
            serde_yaml::from_str(yaml).context("agent front matter is not valid strict YAML")?;
        Ok(Agent {
            front,
            body: body.to_string(),
            origin: String::new(),
        })
    }

    pub fn applies_to(&self, tool: Tool) -> bool {
        self.front.agents.iter().any(|a| a == tool.key())
    }
}

/// One MCP server in `sync/mcp.toml`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct McpServer {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// stdio server: the executable. Exactly one of `command` / `url` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// HTTP (streamable-http) server: the endpoint. Claude renders `{"type":"http","url"}`,
    /// AGY `{"serverUrl"}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default = "both")]
    pub agents: Vec<String>,
}

impl McpServer {
    pub fn applies_to(&self, tool: Tool) -> bool {
        self.agents.iter().any(|a| a == tool.key())
    }

    /// Exactly one transport; an HTTP server takes no process fields.
    pub fn validate(&self, name: &str) -> Result<()> {
        match (&self.command, &self.url) {
            (Some(_), None) => Ok(()),
            (None, Some(_)) if self.args.is_empty() && self.env.is_empty() => Ok(()),
            (None, Some(_)) => bail!("mcp {name}: a url server cannot have args or env"),
            _ => bail!("mcp {name}: set exactly one of command (stdio) or url (http)"),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct McpFile {
    #[serde(default)]
    pub servers: BTreeMap<String, McpServer>,
}

/// One permission direction (allow or deny) in neutral form. Each list keeps the exact Claude
/// matching semantics so the Claude projection is identical to what it replaced.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct PermSet {
    /// Word-bounded prefix: Claude `Bash(X:*)`, AGY `command(X)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shell: Vec<String>,
    /// Raw prefix glob: Claude `Bash(X*)`, AGY `command(X)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shell_glob: Vec<String>,
    /// Exact command: Claude `Bash(X)`, AGY `command(X)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shell_exact: Vec<String>,
    /// Absolute path glob: Claude `Read(/X)` (`//abs`), AGY `read_file(X)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read: Vec<String>,
    /// `server/tool` or `server`: Claude `mcp__server__tool`, AGY `mcp(server/tool)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp: Vec<String>,
    /// Raw entries for one tool only (no equivalent in the other).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claude: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agy: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PermFile {
    #[serde(default)]
    pub allow: PermSet,
    #[serde(default)]
    pub deny: PermSet,
}

/// Per-tool side of a hook. Absent side = the hook is not projected to that tool.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct HookSide {
    /// Tool-specific matcher; `None` = no matcher key (Claude UserPromptSubmit, AGY PreInvocation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Seconds (both tools).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// The hook waits for a person (an approval prompt), so its timeout may go up to
    /// [`MAX_INTERACTIVE_HOOK_TIMEOUT_SECS`] instead of [`MAX_HOOK_TIMEOUT_SECS`]. Explicit per side,
    /// so the milliseconds guard still catches a unit slip everywhere else.
    #[serde(default, skip_serializing_if = "is_false")]
    pub interactive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Hook {
    /// Unique; used as the AGY hooks.json key.
    pub name: String,
    pub event: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<HookSide>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agy: Option<HookSide>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<HookSide>,
}

/// Upper bound for a hook timeout, in seconds. Both tools read SECONDS; a value above this is almost
/// certainly milliseconds, which made a hung guard stall every tool call for over an hour.
pub const MAX_HOOK_TIMEOUT_SECS: u64 = 120;

/// Upper bound for an `interactive` hook (one that waits for a person to answer), in seconds.
/// Still far below any millisecond value a unit slip would produce.
pub const MAX_INTERACTIVE_HOOK_TIMEOUT_SECS: u64 = 900;

fn is_false(b: &bool) -> bool {
    !*b
}

impl Hook {
    /// Reject a timeout that looks like milliseconds, so a unit slip fails `check` and `apply`.
    pub fn validate_timeouts(&self) -> Result<()> {
        for (tool, side) in [
            ("claude", &self.claude),
            ("agy", &self.agy),
            ("codex", &self.codex),
        ] {
            let Some(side) = side.as_ref() else { continue };
            let max = if side.interactive {
                MAX_INTERACTIVE_HOOK_TIMEOUT_SECS
            } else {
                MAX_HOOK_TIMEOUT_SECS
            };
            if let Some(t) = side.timeout {
                if t > max {
                    bail!(
                        "hooks.toml: hook '{}' [{tool}] timeout = {t} exceeds {max} \
                         (timeouts are SECONDS, not milliseconds)",
                        self.name
                    );
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct HookFile {
    #[serde(default, rename = "hook")]
    pub hooks: Vec<Hook>,
}

pub fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// All neutral sources, loaded and validated.
#[derive(Debug, Clone)]
pub struct Sources {
    pub map: MapFile,
    pub agents: Vec<Agent>,
    pub mcp: McpFile,
    pub perms: PermFile,
    pub hooks: HookFile,
    /// `models.toml` is optional: absent means no model-tags projection.
    pub models: Option<crate::models::ModelsFile>,
}

impl Sources {
    /// Load every neutral source under `sync_dir`. `origin` turns an agent file's path into the
    /// label its generated header names.
    pub fn load(sync_dir: &Path, origin: &dyn Fn(&Path) -> String) -> Result<Sources> {
        let map: MapFile = read_toml(&sync_dir.join("map.toml"))?;
        let mut agents = Vec::new();
        let adir = sync_dir.join("agents");
        let mut names: Vec<_> = std::fs::read_dir(&adir)
            .with_context(|| format!("reading {}", adir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "md"))
            .collect();
        names.sort();
        for p in names {
            let text = std::fs::read_to_string(&p)?;
            let mut a = Agent::parse(&text).with_context(|| format!("in {}", p.display()))?;
            a.origin = origin(&p);
            let stem = p.file_stem().map(|s| s.to_string_lossy().to_string());
            if !valid_name(&a.front.name) {
                bail!(
                    "{}: name '{}' must be [A-Za-z0-9._-]+ and not start with a dot",
                    p.display(),
                    a.front.name
                );
            }
            if stem.as_deref() != Some(a.front.name.as_str()) {
                bail!(
                    "{}: name '{}' must match the file name",
                    p.display(),
                    a.front.name
                );
            }
            agents.push(a);
        }
        let hooks: HookFile = read_toml(&sync_dir.join("hooks.toml"))?;
        let mut seen = std::collections::HashSet::new();
        for h in &hooks.hooks {
            if !seen.insert(h.name.clone()) {
                bail!("hooks.toml: duplicate hook name '{}'", h.name);
            }
            h.validate_timeouts()?;
        }
        let mcp: McpFile = read_toml(&sync_dir.join("mcp.toml"))?;
        for (name, srv) in &mcp.servers {
            srv.validate(name)?;
        }
        let models_path = sync_dir.join("models.toml");
        let models = if models_path.is_file() {
            let m: crate::models::ModelsFile = read_toml(&models_path)?;
            m.validate()?;
            Some(m)
        } else {
            None
        };
        Ok(Sources {
            models,
            map,
            agents,
            mcp,
            perms: read_toml(&sync_dir.join("permissions.toml"))?,
            hooks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_timeout_in_milliseconds_is_rejected() {
        let file: HookFile = toml::from_str(
            "[[hook]]\nname = \"g\"\nevent = \"PreToolUse\"\ncommand = \"x\"\n\
             [hook.claude]\ntimeout = 5000\n[hook.agy]\ntimeout = 10\n",
        )
        .unwrap();
        let err = file.hooks[0].validate_timeouts().unwrap_err().to_string();
        assert!(
            err.contains("'g' [claude]") && err.contains("SECONDS"),
            "{err}"
        );
        let ok: HookFile = toml::from_str(
            "[[hook]]\nname = \"g\"\nevent = \"E\"\ncommand = \"x\"\n[hook.claude]\ntimeout = 120\n",
        )
        .unwrap();
        assert!(ok.hooks[0].validate_timeouts().is_ok());
    }

    #[test]
    fn an_interactive_hook_may_wait_longer_but_not_forever() {
        let parse = |agy: &str| -> HookFile {
            toml::from_str(&format!(
                "[[hook]]\nname = \"a\"\nevent = \"PreToolUse\"\ncommand = \"x\"\n[hook.agy]\n{agy}"
            ))
            .unwrap()
        };
        // 600 s is a slip without the flag, fine with it.
        assert!(parse("timeout = 600\n").hooks[0]
            .validate_timeouts()
            .is_err());
        assert!(parse("timeout = 600\ninteractive = true\n").hooks[0]
            .validate_timeouts()
            .is_ok());
        // Milliseconds still fail, flag or not.
        let err = parse("timeout = 600000\ninteractive = true\n").hooks[0]
            .validate_timeouts()
            .unwrap_err()
            .to_string();
        assert!(err.contains("exceeds 900"), "{err}");
        // The flag is ours: it never reaches the hooks file agy reads.
        let rendered =
            crate::render::render_agy_hooks(&parse("timeout = 600\ninteractive = true\n"));
        assert!(!rendered.to_string().contains("interactive"), "{rendered}");
        assert_eq!(rendered["a"]["PreToolUse"][0]["timeout"], 600);
    }

    #[test]
    fn agent_names_are_one_plain_file_name() {
        for ok in ["rust-reviewer", "a.b_c-1", "X"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", ".hidden", "..", "../x", "a/b", "/abs", "a b", "é"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn front_matter_split_and_strict_parse() {
        let a = Agent::parse(
            "---\nname: x\ndescription: \"a: b\"\ntier: fast\ntools: [read, \"mcp:files\"]\nclaude:\n  color: red\n---\nbody\n",
        )
        .unwrap();
        assert_eq!(a.front.description, "a: b");
        assert_eq!(a.front.tools, vec!["read", "mcp:files"]);
        assert_eq!(a.front.agents, vec!["claude", "agy"]);
        assert_eq!(a.body, "body\n");
        // Unquoted ": " is rejected — the same strictness AGY applies.
        assert!(Agent::parse("---\nname: x\ndescription: a: b\ntier: fast\n---\n").is_err());
    }
}
