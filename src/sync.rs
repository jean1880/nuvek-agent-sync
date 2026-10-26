//! Surfaces: every place a projection lands. Each surface can be checked (read-only diff of the
//! rendered form against disk) and applied (atomic write, merge where the tool co-owns the file).

use crate::model::{Sources, Tool};
use crate::render;
use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use toml_edit::{value as toml_value, Array as TomlArray, DocumentMut, InlineTable};

pub struct Paths {
    pub home: PathBuf,
    /// Where every link points: the source checkout, or with an overlay the stable resolved tree.
    pub ac: PathBuf,
    /// Where sources are read from. Equal to `ac`, except that `check` with an overlay reads a
    /// scratch resolution so it never writes the state directory.
    pub src: PathBuf,
    /// The checkouts behind `ac`: the source alone, or the base then the overlay.
    pub layers: Vec<PathBuf>,
    /// agent-sync's own state: the lock, the managed-destinations manifest, the resolved tree.
    pub state: PathBuf,
    pub claude: PathBuf,
    pub gemini: PathBuf,
    pub codex: PathBuf,
    pub claude_json: PathBuf,
    pub git_root: PathBuf,
    /// XDG config home (`~/.config` unless the caller overrides it from the environment).
    pub config: PathBuf,
}

impl Paths {
    pub fn from_home(home: &Path, ac: Option<PathBuf>) -> Paths {
        let ac = ac.unwrap_or_else(|| home.join("git/agent-config"));
        Paths {
            home: home.to_path_buf(),
            src: ac.clone(),
            layers: vec![ac.clone()],
            state: home.join(".local/state/agent-sync"),
            ac,
            claude: home.join(".claude"),
            gemini: home.join(".gemini"),
            codex: home.join(".codex"),
            claude_json: home.join(".claude.json"),
            git_root: home.join("git"),
            config: home.join(".config"),
        }
    }
    /// Honour `XDG_CONFIG_HOME` (kept out of `from_home` so tests on a temp home never see it).
    pub fn with_xdg_config(mut self, xdg: Option<String>) -> Paths {
        if let Some(x) = xdg.filter(|x| Path::new(x).is_absolute()) {
            self.config = PathBuf::from(x);
        }
        self
    }
    pub fn model_tags(&self) -> PathBuf {
        self.config.join("agent-terminal/model-tags.json")
    }
    /// The neutral sources, as read.
    pub fn sync(&self) -> PathBuf {
        self.src.join("sync")
    }
    /// True when an overlay is layered over the base.
    pub fn layered(&self) -> bool {
        self.layers.len() > 1
    }
    /// The source checkouts as the generated headers name them (`base + overlay`).
    pub fn source_label(&self) -> String {
        if self.layered() {
            self.layers
                .iter()
                .map(|l| self.display(l))
                .collect::<Vec<_>>()
                .join(" + ")
        } else {
            self.display(&self.ac)
        }
    }
    /// `path` as a person reads it: `~/<rest>` under the home directory, absolute otherwise.
    pub fn display(&self, path: &Path) -> String {
        display_home(&self.home, path)
    }
    /// Load the neutral sources; each agent's header names its source file (with an overlay,
    /// the layer file the resolved link points at).
    pub fn sources(&self) -> Result<Sources> {
        let layered = self.layered();
        Sources::load(&self.sync(), &|path| {
            let origin = if layered {
                std::fs::read_link(path).unwrap_or_else(|_| path.to_path_buf())
            } else {
                path.to_path_buf()
            };
            self.display(&origin)
        })
    }
    /// Allow writes into the directories agent-sync manages, and nowhere else (see `fsx`). The
    /// source's `sync/` is writable only without an overlay (`import`, `promote --add`).
    pub fn register_write_roots(&self) {
        let mut roots = vec![
            self.claude.clone(),
            self.gemini.clone(),
            self.codex.clone(),
            self.config.join("agent-terminal"),
            self.state.clone(),
        ];
        if !self.layered() {
            roots.push(self.ac.join("sync"));
        }
        for repo in overlay_repos(self) {
            roots.push(repo.join(".agents"));
            roots.push(repo.join(".git/info"));
        }
        let mut dropped = crate::fsx::allow_writes_under(roots);
        if !crate::fsx::allow_file(&self.claude_json) {
            dropped.push(self.claude_json.clone());
        }
        for d in dropped {
            say_err!(
                "agent-sync: warning: {} cannot be resolved (a dangling or looping link?); \
                 nothing under it will be written",
                d.display()
            );
        }
    }
    fn manifest_path(&self) -> PathBuf {
        self.state.join("managed.json")
    }
    fn agy_settings(&self) -> PathBuf {
        self.gemini.join("antigravity-cli/settings.json")
    }
}

/// `~/<rest>` when `path` is under `home`, the path as given otherwise.
pub fn display_home(home: &Path, path: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    /// Tool-appended approvals not (yet) in permissions.toml. Warning only.
    Unpromoted,
    Drift,
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub level: Level,
    pub surface: String,
    pub detail: String,
}

fn f(level: Level, surface: impl Into<String>, detail: impl Into<String>) -> Finding {
    Finding {
        level,
        surface: surface.into(),
        detail: detail.into(),
    }
}

// ---------- low-level I/O ----------

/// Refuse to write anything secret-shaped (fail-closed: this is the last gate before disk).
pub fn assert_clean(what: &str, text: &str) -> Result<()> {
    if nuvek_core::scrub::has_secret(text) {
        bail!("refusing to write {what}: rendered content contains a secret-shaped token");
    }
    Ok(())
}

/// Refuse secret-shaped content, then write it with [`crate::fsx::atomic_write`]: exclusive
/// temporaries renamed into place, the previous file kept as `.agent-sync.bak`.
pub fn atomic_write(path: &Path, content: &str) -> Result<()> {
    assert_clean(&path.display().to_string(), content)?;
    crate::fsx::atomic_write(path, content)
}

fn read_json(path: &Path) -> Result<Value> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// `None` when the file does not exist (a fresh machine); any other read or parse failure is an
/// error, never an empty document.
fn read_json_opt(path: &Path) -> Result<Option<Value>> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        _ => read_json(path).map(Some),
    }
}

/// A tool file that `check` expected but did not find: drift, so a fresh machine exits 6.
fn missing_file(p: &Paths, path: &Path) -> Finding {
    f(Level::Drift, format!("file {}", p.display(path)), "missing")
}

fn pretty(v: &Value) -> Result<String> {
    Ok(format!("{}\n", serde_json::to_string_pretty(v)?))
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Refusal to write a file AGY co-owns while an agy process could rewrite it. Typed, so callers
/// map it to exit 5 without matching on message text.
#[derive(Debug)]
pub struct AgyRunning(pub String);

impl std::fmt::Display for AgyRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "agy is running (it rewrites {}); close it or pass --force",
            self.0
        )
    }
}

impl std::error::Error for AgyRunning {}

/// FAIL-CLOSED: if pgrep cannot answer (missing, error, timeout), assume agy IS running — the
/// cost is a refusal the user overrides with --force, never a clobbered AGY settings file.
pub fn agy_running() -> bool {
    let mut cmd = Command::new("pgrep");
    cmd.args(["-x", "agy"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match nuvek_core::util::command_output_timeout(cmd, Duration::from_secs(5)) {
        Ok(Some(o)) => o.status.code() != Some(1), // pgrep: 0 = found, 1 = none
        _ => true,
    }
}

/// Optional shell-style env file (`NAME=value` / `export NAME="value"`) read for placeholder
/// values that are not in the process environment.
pub const ENV_FILE_VAR: &str = "AGENT_SYNC_ENV_FILE";

/// A placeholder's value: the process environment first, then `$AGENT_SYNC_ENV_FILE` if set.
fn env_lookup(name: &str) -> Option<String> {
    nuvek_core::env::env_var(name).or_else(|| {
        let path = nuvek_core::env::expand_home(&nuvek_core::env::env_var(ENV_FILE_VAR)?);
        nuvek_core::env::read_env_file(&path, name).ok().flatten()
    })
}

// ---------- resolved source tree (overlay mode) ----------

/// With an overlay, `check` resolves into a scratch tree: the installed tree must match it.
/// Nothing to report without an overlay, or when the sources are read from the installed tree.
pub fn check_resolved(p: &Paths) -> Vec<Finding> {
    if !p.layered() || p.src == p.ac {
        return Vec::new();
    }
    let s = format!("resolved source tree {}", p.display(&p.ac));
    vec![if !p.ac.exists() {
        f(Level::Drift, s, "missing")
    } else if crate::overlay::same_tree(&p.ac, &p.src) {
        f(Level::Ok, s, "")
    } else {
        f(
            Level::Drift,
            s,
            "differs from the base and overlay checkouts",
        )
    }]
}

// ---------- links ----------

/// The Claude plugin's name, from the source's `.claude-plugin/plugin.json`. It names the
/// `~/.claude/skills/<name>` and `~/.claude/plugins/<name>` links, so it must be one plain path
/// component.
pub fn plugin_name(p: &Paths) -> Result<String> {
    let path = p.src.join(".claude-plugin/plugin.json");
    let v = read_json(&path)?;
    let name = v
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let plain = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\0');
    if !plain {
        bail!(
            "{}: `name` must be a non-empty single path component",
            path.display()
        );
    }
    Ok(name.to_string())
}

/// (link path, expected target). The CLAUDE.md import file is checked separately.
fn expected_links(p: &Paths) -> Vec<(PathBuf, PathBuf)> {
    let mut links = Vec::new();
    if let Ok(name) = plugin_name(p) {
        links.push((p.claude.join("skills").join(&name), p.ac.clone()));
        links.push((p.claude.join("plugins").join(&name), p.ac.clone()));
    }
    links.extend([
        (p.claude.join("commands"), p.ac.join("sync/commands")),
        (p.gemini.join("AGENTS.md"), p.ac.join("rules/core.md")),
        (p.gemini.join("GEMINI.md"), p.ac.join("rules/agy.md")),
        (p.gemini.join("config/skills"), p.ac.join("skills")),
        (
            p.gemini.join("config/rules/memory-pointer.md"),
            p.ac.join("rules/memory-pointer.md"),
        ),
    ]);
    links.extend(
        std::fs::read_dir(p.src.join("skills"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name();
                (entry.path().join("SKILL.md").is_file()).then(|| {
                    (
                        p.codex.join("skills").join(&name),
                        p.ac.join("skills").join(&name),
                    )
                })
            }),
    );
    links
}

/// Load and render every projection from `p.src` without writing anything: a candidate tree
/// that fails here never goes live.
pub fn validate_sources(p: &Paths) -> Result<()> {
    plugin_name(p)?;
    codex_md(p).context("rendering ~/.codex/AGENTS.md")?;
    let src = p.sources()?;
    for tool in [Tool::Claude, Tool::Agy] {
        desired_agents(&src, tool)?;
        desired_mcp(&src, tool)?;
    }
    desired_codex_mcp(&src)?;
    if let Some(models) = &src.models {
        models.render()?;
    }
    Ok(())
}

// ---------- managed destinations ----------

/// What agent-sync owns, recorded at the end of every `apply` in `<state>/managed.json`. Only a
/// destination listed here is ever deleted. With no manifest yet (the first run), nothing is
/// deleted: the run adopts the links and generated files that match what it creates.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    pub version: u32,
    /// link path → the target agent-sync gave it.
    #[serde(default)]
    pub links: BTreeMap<PathBuf, PathBuf>,
    /// Generated files (rendered agents).
    #[serde(default)]
    pub files: std::collections::BTreeSet<PathBuf>,
}

impl Manifest {
    /// `None` before the first apply.
    pub fn load(p: &Paths) -> Result<Option<Manifest>> {
        let path = p.manifest_path();
        let remedy = || {
            format!(
                "{} is unusable; delete it to start fresh (the next apply then deletes nothing \
                 and records what it creates)",
                path.display()
            )
        };
        crate::fsx::refuse_symlink(&path).with_context(remedy)?;
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(remedy),
        };
        let m: Manifest = serde_json::from_str(&text).with_context(remedy)?;
        if m.version != MANIFEST_VERSION {
            return Err(anyhow::anyhow!("unknown manifest version {}", m.version))
                .with_context(remedy);
        }
        Ok(Some(m))
    }
}

const MANIFEST_VERSION: u32 = 1;

/// Update the manifest: keep earlier entries that are still in place (so a stale link stays
/// listed until it is pruned), and add every expected link that is in place and every rendered
/// agent file. Called after each stage of `apply`, so a run that stops partway still records
/// what it created.
pub fn record_managed(p: &Paths, src: &Sources) -> Result<()> {
    let mut m = Manifest::load(p)?.unwrap_or_default();
    m.version = MANIFEST_VERSION;
    m.links
        .retain(|link, target| std::fs::read_link(link).is_ok_and(|t| &t == target));
    m.files.retain(|f| f.is_file());
    for (link, target) in expected_links(p) {
        if std::fs::read_link(&link).is_ok_and(|t| t == target) {
            m.links.insert(link, target);
        }
    }
    for tool in [Tool::Claude, Tool::Agy] {
        let dir = agents_dir(p, tool);
        for name in desired_agents(src, tool)?.keys() {
            let path = dir.join(name);
            if path.is_file() {
                m.files.insert(path);
            }
        }
    }
    crate::fsx::refuse_symlink(&p.state)?;
    atomic_write(&p.manifest_path(), &pretty(&serde_json::to_value(&m)?)?)
}

/// Add what a stage created to the manifest in one atomic write. Stages call this whether they
/// finished or failed, so everything created before a failure is recorded.
fn record_created(p: &Paths, links: &[(PathBuf, PathBuf)], files: &[PathBuf]) -> Result<()> {
    if links.is_empty() && files.is_empty() {
        return Ok(());
    }
    let mut m = Manifest::load(p)?.unwrap_or_default();
    m.version = MANIFEST_VERSION;
    for (l, t) in links {
        m.links.insert(l.clone(), t.clone());
    }
    m.files.extend(files.iter().cloned());
    crate::fsx::refuse_symlink(&p.state)?;
    atomic_write(&p.manifest_path(), &pretty(&serde_json::to_value(&m)?)?)
}

/// Run a stage, then record what it created, whatever its outcome. A failure to record is
/// reported after the stage's own error, never instead of it.
fn recording<F>(p: &Paths, stage: F) -> Result<()>
where
    F: FnOnce(&mut Vec<(PathBuf, PathBuf)>, &mut Vec<PathBuf>) -> Result<()>,
{
    let (mut links, mut files) = (Vec::new(), Vec::new());
    let result = stage(&mut links, &mut files);
    let recorded = record_created(p, &links, &files);
    match (result, recorded) {
        (Err(e), Err(r)) => Err(e.context(format!("also failed to record it: {r:#}"))),
        (Err(e), Ok(())) => Err(e),
        (Ok(()), r) => r,
    }
}

/// Links in the manifest that no expected link accounts for any more, and that still point
/// where agent-sync pointed them (anything changed by hand since is left alone).
fn stale_links(p: &Paths, expected: &[(PathBuf, PathBuf)]) -> Result<Vec<PathBuf>> {
    let Some(m) = Manifest::load(p)? else {
        return Ok(Vec::new());
    };
    Ok(m.links
        .iter()
        .filter(|(link, _)| !expected.iter().any(|(want, _)| want == *link))
        .filter(|(link, target)| std::fs::read_link(link).is_ok_and(|t| &t == *target))
        .map(|(link, _)| link.clone())
        .collect())
}

fn claude_md(p: &Paths) -> String {
    format!(
        "# Claude Global Context\n@{}\n@{}\n",
        p.ac.join("rules/core.md").display(),
        p.ac.join("rules/claude.md").display()
    )
}

fn codex_md(p: &Paths) -> Result<String> {
    let core = std::fs::read_to_string(p.src.join("rules/core.md"))?;
    let overlay = std::fs::read_to_string(p.src.join("rules/codex.md"))?;
    Ok(format!(
        "<!-- {}{} -->\n{core}\n{overlay}",
        render::GENERATED_MARK,
        p.source_label()
    ))
}

pub fn check_links(p: &Paths) -> Vec<Finding> {
    let mut out = Vec::new();
    if let Err(e) = plugin_name(p) {
        out.push(f(Level::Drift, "claude plugin", format!("{e:#}")));
    }
    for (link, target) in expected_links(p) {
        let s = format!("link {}", link.display());
        match std::fs::read_link(&link) {
            Ok(t) if t == target => out.push(f(Level::Ok, s, "")),
            Ok(t) => out.push(f(
                Level::Drift,
                s,
                format!("points at {} (want {})", t.display(), target.display()),
            )),
            Err(_) if link.exists() => out.push(f(
                Level::Drift,
                s,
                "is a real file/dir, not a link (move it aside by hand)",
            )),
            Err(_) => out.push(f(Level::Drift, s, "missing")),
        }
    }
    match stale_links(p, &expected_links(p)) {
        Ok(stale) => {
            for link in stale {
                out.push(f(
                    Level::Drift,
                    format!("link {}", link.display()),
                    "stale (managed, no longer in the source; apply removes it)",
                ));
            }
        }
        Err(e) => out.push(f(Level::Drift, "managed.json", format!("{e:#}"))),
    }
    let cm = p.claude.join("CLAUDE.md");
    let want = claude_md(p);
    match std::fs::read_to_string(&cm) {
        Ok(t) if t == want => out.push(f(Level::Ok, "file ~/.claude/CLAUDE.md", "")),
        _ => out.push(f(
            Level::Drift,
            "file ~/.claude/CLAUDE.md",
            "must be exactly the two @imports",
        )),
    }
    let codex = p.codex.join("AGENTS.md");
    match (std::fs::read_to_string(&codex), codex_md(p)) {
        (Ok(have), Ok(want)) if have == want => {
            out.push(f(Level::Ok, "file ~/.codex/AGENTS.md", ""))
        }
        (Ok(_), Ok(_)) => out.push(f(
            Level::Drift,
            "file ~/.codex/AGENTS.md",
            "generated content differs from rules/core.md + rules/codex.md",
        )),
        (Err(_), _) => out.push(f(Level::Drift, "file ~/.codex/AGENTS.md", "missing")),
        (_, Err(e)) => out.push(f(
            Level::Drift,
            "file ~/.codex/AGENTS.md",
            format!("cannot render: {e:#}"),
        )),
    }
    out
}

pub fn apply_links(p: &Paths, log: &crate::out::Log) -> Result<()> {
    plugin_name(p)?;
    let expected = expected_links(p);
    // Refuse before changing anything when a real file sits where a link belongs.
    for (link, _) in &expected {
        if std::fs::symlink_metadata(link).is_ok_and(|m| !m.file_type().is_symlink()) {
            bail!(
                "{} is a real file/dir; move it aside by hand first",
                link.display()
            );
        }
    }
    for link in stale_links(p, &expected)? {
        if crate::fsx::remove_link(&link)? {
            log.info(&format!("removed stale link {}", link.display()));
        }
    }
    recording(p, |made, _| {
        for (link, target) in expected {
            if std::fs::read_link(&link).is_ok_and(|t| t == target) {
                continue;
            }
            crate::fsx::replace_link(&link, &target)?;
            log.info(&format!(
                "linked {} -> {}",
                link.display(),
                target.display()
            ));
            made.push((link, target));
        }
        Ok(())
    })?;
    let cm = p.claude.join("CLAUDE.md");
    if std::fs::read_to_string(&cm).ok().as_deref() != Some(claude_md(p).as_str()) {
        atomic_write(&cm, &claude_md(p))?;
        log.info("rewrote ~/.claude/CLAUDE.md imports");
    }
    let codex = p.codex.join("AGENTS.md");
    let generated = codex_md(p)?;
    if std::fs::read_to_string(&codex).ok().as_deref() != Some(generated.as_str()) {
        atomic_write(&codex, &generated)?;
        log.info("rewrote ~/.codex/AGENTS.md from shared rules");
    }
    Ok(())
}

// ---------- skills (check only) ----------

pub fn check_skills(p: &Paths) -> Vec<Finding> {
    let mut out = Vec::new();
    let dir = p.src.join("skills");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return vec![f(
            Level::Drift,
            "skills",
            format!("cannot read {}", dir.display()),
        )];
    };
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let md = e.path().join("SKILL.md");
        if !md.is_file() {
            continue;
        }
        n += 1;
        let s = format!("skill {name}");
        let parsed = std::fs::read_to_string(&md)
            .map_err(anyhow::Error::from)
            .and_then(|t| {
                let (yaml, _) = crate::model::split_front_matter(&t)?;
                Ok(serde_yaml::from_str::<serde_yaml::Value>(yaml)?)
            });
        match parsed {
            Ok(v) if v["name"].as_str() == Some(name.as_str()) && v["description"].is_string() => {}
            Ok(_) => out.push(f(
                Level::Drift,
                s,
                "front matter needs name == folder and a string description",
            )),
            Err(e) => out.push(f(
                Level::Drift,
                s,
                format!("front matter is not strict YAML (AGY drops the skill): {e:#}"),
            )),
        }
    }
    out.push(f(Level::Ok, "skills", format!("{n} skills, strict YAML")));
    out
}

// ---------- agents ----------

fn agents_dir(p: &Paths, tool: Tool) -> PathBuf {
    match tool {
        Tool::Claude => p.claude.join("agents"),
        Tool::Agy => p.gemini.join("config/agents"),
    }
}

fn desired_agents(src: &Sources, tool: Tool) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for a in src.agents.iter().filter(|a| a.applies_to(tool)) {
        out.insert(
            format!("{}.md", a.front.name),
            render::render_agent(&src.map, a, tool)
                .with_context(|| format!("agent {}", a.front.name))?,
        );
    }
    Ok(out)
}

fn existing_md(dir: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.ends_with(".md") {
                if let Ok(t) = std::fs::read_to_string(e.path()) {
                    out.insert(n, t);
                }
            }
        }
    }
    out
}

pub fn check_agents(p: &Paths, src: &Sources, tool: Tool) -> Result<Vec<Finding>> {
    let want = desired_agents(src, tool)?;
    let have = existing_md(&agents_dir(p, tool));
    let mut out = Vec::new();
    for (n, w) in &want {
        let s = format!("{} agent {n}", tool.key());
        match have.get(n) {
            Some(h) if h == w => out.push(f(Level::Ok, s, "")),
            Some(h) => {
                // If the file is not identical, check if it's invalid schema or just drifted content
                if let Ok((yaml, _)) = crate::model::split_front_matter(h) {
                    if let Err(e) = render::validate_agent_frontmatter(yaml, tool) {
                        out.push(f(Level::Drift, s, format!("invalid schema: {e}")));
                        continue;
                    }
                }
                out.push(f(Level::Drift, s, line_delta(w, h)));
            }
            None => out.push(f(Level::Drift, s, "missing")),
        }
    }
    let managed = Manifest::load(p)?.map(|m| m.files).unwrap_or_default();
    let dir = agents_dir(p, tool);
    for (n, h) in &have {
        if !want.contains_key(n) {
            let why = match orphan(&managed, &dir.join(n), h) {
                Orphan::Managed => "orphan (generated, no source)",
                Orphan::Unrecorded => {
                    "generated, but not recorded in managed.json (remove it by hand)"
                }
                Orphan::Foreign => "unmanaged (no source in sync/agents)",
            };
            out.push(f(Level::Drift, format!("{} agent {n}", tool.key()), why));
        }
    }
    Ok(out)
}

enum Orphan {
    /// agent-sync wrote it (listed in the manifest): `apply` deletes it.
    Managed,
    /// It carries a generated header but is not listed: never deleted automatically.
    Unrecorded,
    /// Someone else's file.
    Foreign,
}

/// The generated header is its own line after the front matter, never a substring elsewhere.
fn has_generated_header(text: &str) -> bool {
    let body = crate::model::split_front_matter(text)
        .map(|(_, b)| b)
        .unwrap_or("");
    body.lines()
        .next()
        .is_some_and(|l| l.starts_with(&format!("<!-- {}", render::GENERATED_MARK)))
}

/// Deletable only when agent-sync recorded the file AND it still carries the generated header:
/// a file someone replaced by hand is theirs, whatever the manifest says.
fn orphan(managed: &std::collections::BTreeSet<PathBuf>, path: &Path, text: &str) -> Orphan {
    match (has_generated_header(text), managed.contains(path)) {
        (true, true) => Orphan::Managed,
        (true, false) => Orphan::Unrecorded,
        (false, _) => Orphan::Foreign,
    }
}

pub fn apply_agents(p: &Paths, src: &Sources, tool: Tool, log: &crate::out::Log) -> Result<()> {
    let dir = agents_dir(p, tool);
    let want = desired_agents(src, tool)?;
    let have = existing_md(&dir);
    recording(p, |_, made| {
        for (n, w) in &want {
            if have.get(n) != Some(w) {
                atomic_write(&dir.join(n), w)?;
                log.info(&format!("wrote {} agent {n}", tool.key()));
                made.push(dir.join(n));
            }
        }
        Ok(())
    })?;
    let managed = Manifest::load(p)?.map(|m| m.files).unwrap_or_default();
    for (n, h) in &have {
        if !want.contains_key(n) {
            match orphan(&managed, &dir.join(n), h) {
                Orphan::Managed => {
                    crate::fsx::remove_file(&dir.join(n))?;
                    log.info(&format!("removed orphan {} agent {n}", tool.key()));
                }
                Orphan::Unrecorded => log.warn(&format!(
                    "{} agent {n} looks generated but is not in managed.json; left in place",
                    tool.key()
                )),
                Orphan::Foreign => bail!(
                    "{} agent {n} has no source and is not generated; add it to sync/agents or \
                     remove it by hand",
                    tool.key()
                ),
            }
        }
    }
    Ok(())
}

/// Compact line-level delta (lines only in want: `+`, only in have: `-`), capped.
fn line_delta(want: &str, have: &str) -> String {
    let w: Vec<&str> = want.lines().collect();
    let h: Vec<&str> = have.lines().collect();
    let w_set: std::collections::HashSet<&str> = w.iter().copied().collect();
    let h_set: std::collections::HashSet<&str> = h.iter().copied().collect();
    let mut d: Vec<String> = Vec::new();
    d.extend(
        w.iter()
            .filter(|l| !h_set.contains(*l))
            .map(|l| format!("+ {l}")),
    );
    d.extend(
        h.iter()
            .filter(|l| !w_set.contains(*l))
            .map(|l| format!("- {l}")),
    );
    let n = d.len();
    d.truncate(8);
    let mut s = d.join("\n      ");
    if n > 8 {
        s.push_str(&format!("\n      … {} more", n - 8));
    }
    if s.is_empty() {
        s = "differs in line order/whitespace".into();
    }
    format!("content differs:\n      {s}")
}

// ---------- MCP ----------

fn desired_mcp(src: &Sources, tool: Tool) -> Result<Map<String, Value>> {
    render::render_mcp(&src.mcp, tool, &src.map.placeholders, &env_lookup)
}

fn desired_codex_mcp(src: &Sources) -> Result<Map<String, Value>> {
    let mut out = Map::new();
    for (name, server) in &src.mcp.servers {
        if server.agents.iter().any(|a| a == "codex") {
            out.insert(
                name.clone(),
                render::render_mcp_server(
                    server,
                    Tool::Claude,
                    &src.map.placeholders,
                    &env_lookup,
                )?,
            );
        }
    }
    Ok(out)
}

fn codex_server_item(server: &Value) -> Result<toml_edit::Item> {
    let mut doc = DocumentMut::new();
    if let Some(url) = server.get("url").and_then(Value::as_str) {
        doc["url"] = toml_value(url);
    } else {
        if let Some(command) = server.get("command").and_then(Value::as_str) {
            doc["command"] = toml_value(command);
        }
        if let Some(args) = server.get("args").and_then(Value::as_array) {
            let mut values = TomlArray::new();
            for arg in args.iter().filter_map(Value::as_str) {
                values.push(arg);
            }
            doc["args"] = toml_value(values);
        }
        if let Some(env) = server.get("env").and_then(Value::as_object) {
            let mut values = InlineTable::new();
            for (key, val) in env {
                if let Some(val) = val.as_str() {
                    values.insert(key, toml_edit::Value::from(val));
                }
            }
            doc["env"] = toml_edit::Item::Value(toml_edit::Value::InlineTable(values));
        }
    }
    Ok(toml_edit::Item::Table(doc.as_table().clone()))
}

/// `None` when `~/.codex/config.toml` does not exist yet.
fn codex_mcp_doc(p: &Paths) -> Result<Option<DocumentMut>> {
    let path = p.codex.join("config.toml");
    let text = match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        r => r.with_context(|| format!("reading {}", path.display()))?,
    };
    text.parse::<DocumentMut>()
        .map(Some)
        .with_context(|| format!("parsing {}", path.display()))
}

fn mcp_servers_of(v: Option<&Value>) -> Map<String, Value> {
    v.and_then(|v| v.get("mcpServers"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

pub fn check_mcp(p: &Paths, src: &Sources) -> Result<Vec<Finding>> {
    let mut out = Vec::new();
    // Claude: ~/.claude.json is read-only here; apply goes through `claude mcp`.
    let want = desired_mcp(src, Tool::Claude)?;
    let claude_json = read_json_opt(&p.claude_json)?;
    if claude_json.is_none() {
        out.push(missing_file(p, &p.claude_json));
    }
    let have = mcp_servers_of(claude_json.as_ref());
    diff_servers("claude", &want, &have, &mut out);
    // AGY: generated file.
    let want = desired_mcp(src, Tool::Agy)?;
    let path = p.gemini.join("config/mcp_config.json");
    let have = read_json(&path)
        .ok()
        .and_then(|v| v.get("mcpServers").and_then(Value::as_object).cloned())
        .unwrap_or_default();
    diff_servers("agy", &want, &have, &mut out);
    let want = desired_codex_mcp(src)?;
    let doc = codex_mcp_doc(p)?;
    if doc.is_none() {
        out.push(missing_file(p, &p.codex.join("config.toml")));
    }
    let have = doc
        .as_ref()
        .and_then(|d| d.get("mcp_servers"))
        .and_then(toml_edit::Item::as_table);
    for (name, server) in &want {
        let expected = codex_server_item(server)?;
        let current = have.and_then(|table| table.get(name));
        out.push(
            if current.is_some_and(|item| item.to_string() == expected.to_string()) {
                f(Level::Ok, format!("codex mcp {name}"), "")
            } else {
                f(
                    Level::Drift,
                    format!("codex mcp {name}"),
                    "definition differs or is not registered",
                )
            },
        );
    }
    for name in codex_extra_servers(have, &want) {
        out.push(f(
            Level::Drift,
            format!("codex mcp {name}"),
            "registered but not in mcp.toml (apply --prune removes)",
        ));
    }
    Ok(out)
}

/// Codex servers registered in config.toml that mcp.toml no longer declares.
fn codex_extra_servers(have: Option<&toml_edit::Table>, want: &Map<String, Value>) -> Vec<String> {
    have.map(|table| {
        table
            .iter()
            .map(|(name, _)| name.to_string())
            .filter(|name| !want.contains_key(name))
            .collect()
    })
    .unwrap_or_default()
}

/// Replace a Codex server's definition but keep its Codex-native `tools` table (per-tool
/// `approval_mode`): agent-sync owns the launch definition, not the user's tool approvals.
fn replace_codex_server(servers: &mut toml_edit::Table, name: &str, mut item: toml_edit::Item) {
    if let Some(tools) = servers
        .get(name)
        .and_then(|have| have.get("tools"))
        .cloned()
    {
        if let Some(table) = item.as_table_mut() {
            table.insert("tools", tools);
        }
    }
    servers.insert(name, item);
}

fn diff_servers(
    tool: &str,
    want: &Map<String, Value>,
    have: &Map<String, Value>,
    out: &mut Vec<Finding>,
) {
    for (n, w) in want {
        let s = format!("{tool} mcp {n}");
        match have.get(n) {
            Some(h) if render::normalize_server(h) == *w => out.push(f(Level::Ok, s, "")),
            Some(_) => out.push(f(Level::Drift, s, "definition differs")),
            None => out.push(f(Level::Drift, s, "not registered")),
        }
    }
    for n in have.keys().filter(|n| !want.contains_key(*n)) {
        out.push(f(
            Level::Drift,
            format!("{tool} mcp {n}"),
            "registered but not in mcp.toml (apply --prune removes)",
        ));
    }
}

/// The Claude Code CLI that `apply` registers MCP servers through (default `claude` on PATH).
pub const CLAUDE_BIN_VAR: &str = "AGENT_SYNC_CLAUDE_BIN";

fn claude_cli(args: &[&str]) -> Result<()> {
    let bin = nuvek_core::env::env_var(CLAUDE_BIN_VAR).unwrap_or_else(|| "claude".into());
    let mut cmd = Command::new(bin);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    match nuvek_core::util::command_output_timeout(cmd, Duration::from_secs(60))? {
        Some(o) if o.status.success() => Ok(()),
        Some(o) => bail!(
            "claude {} failed: {}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        None => bail!("claude {} timed out", args.first().copied().unwrap_or("")),
    }
}

pub fn apply_mcp(p: &Paths, src: &Sources, prune: bool, log: &crate::out::Log) -> Result<()> {
    let want = desired_mcp(src, Tool::Claude)?;
    // `claude mcp` rewrites ~/.claude.json: the same destination policy as our own writes
    // applies before the CLI is ever run (no symlink, inside the managed paths).
    crate::fsx::check_destination(&p.claude_json)?;
    let claude_json = read_json_opt(&p.claude_json)?;
    if claude_json.is_none() {
        // A fresh machine: seed a document with no servers, which Claude Code fills in on first
        // start, so `check` converges even when there is nothing to register.
        atomic_write(&p.claude_json, &pretty(&json!({ "mcpServers": {} }))?)?;
        log.info("created ~/.claude.json with no MCP servers");
    }
    let have = mcp_servers_of(claude_json.as_ref());
    for (n, w) in &want {
        if have.get(n).map(render::normalize_server).as_ref() == Some(w) {
            continue;
        }
        let js = serde_json::to_string(w)?;
        // The name goes on the CLI's command line too.
        assert_clean("an mcp server name", n)?;
        assert_clean(&format!("mcp server {n}"), &js)?;
        if have.contains_key(n) {
            claude_cli(&["mcp", "remove", "-s", "user", n])?;
        }
        claude_cli(&["mcp", "add-json", "-s", "user", n, &js])?;
        log.info(&format!("claude mcp {n}: registered"));
    }
    if prune {
        for n in have.keys().filter(|n| !want.contains_key(*n)) {
            // A name is put on the CLI's command line: one shaped like a secret never is.
            if nuvek_core::scrub::has_secret(n) {
                log.warn(&format!(
                    "claude mcp {n}: not pruned, the name looks like a secret; remove it by hand"
                ));
                say_err!("agent-sync: warning: an MCP server whose name looks like a secret ({n}) was not pruned; remove it by hand");
                continue;
            }
            claude_cli(&["mcp", "remove", "-s", "user", n])?;
            log.info(&format!("claude mcp {n}: pruned"));
        }
    }
    let agy = json!({ "mcpServers": Value::Object(desired_mcp(src, Tool::Agy)?) });
    let path = p.gemini.join("config/mcp_config.json");
    if read_json(&path).ok().as_ref() != Some(&agy) {
        atomic_write(&path, &pretty(&agy)?)?;
        log.info("wrote agy mcp_config.json");
    }
    let existing = codex_mcp_doc(p)?;
    let codex_exists = existing.is_some();
    let mut codex = existing.unwrap_or_default();
    let codex_before = codex.to_string();
    let codex_servers = codex
        .entry("mcp_servers")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
    let codex_servers = codex_servers
        .as_table_mut()
        .context("Codex mcp_servers is not a table")?;
    let codex_want = desired_codex_mcp(src)?;
    for (name, server) in &codex_want {
        let item = codex_server_item(server)?;
        if !codex_servers
            .get(name)
            .is_some_and(|have| have.to_string() == item.to_string())
        {
            replace_codex_server(codex_servers, name, item);
            log.info(&format!("codex mcp {name}: registered"));
        }
    }
    if prune {
        for name in codex_extra_servers(Some(codex_servers), &codex_want) {
            codex_servers.remove(&name);
            log.info(&format!("codex mcp {name}: pruned"));
        }
    }
    let codex_after = codex.to_string();
    if codex_after != codex_before || !codex_exists {
        atomic_write(&p.codex.join("config.toml"), &codex_after)?;
    }
    Ok(())
}

// ---------- permissions ----------

pub struct PermState {
    pub findings: Vec<Finding>,
    /// Entries the tools appended that are not in permissions.toml: (tool, file, entry).
    pub unpromoted: Vec<(Tool, PathBuf, String)>,
}

pub fn check_perms(p: &Paths, src: &Sources) -> Result<PermState> {
    let mut findings = Vec::new();
    let mut unpromoted = Vec::new();
    // Claude settings.json: fully owned.
    let cs_path = p.claude.join("settings.json");
    let cs = match read_json_opt(&cs_path)? {
        Some(v) => v,
        None => {
            findings.push(missing_file(p, &cs_path));
            json!({})
        }
    };
    for (key, set) in [("allow", &src.perms.allow), ("deny", &src.perms.deny)] {
        let want = render::render_perms(set, Tool::Claude);
        let have = str_list(cs.pointer(&format!("/permissions/{key}")));
        let want_set: std::collections::HashSet<&str> = want.iter().map(String::as_str).collect();
        let have_set: std::collections::HashSet<&str> = have.iter().map(String::as_str).collect();
        let missing: Vec<_> = want
            .iter()
            .filter(|w| !have_set.contains(w.as_str()))
            .collect();
        let extra: Vec<_> = have
            .iter()
            .filter(|h| !want_set.contains(h.as_str()))
            .collect();
        let s = format!("claude settings.json permissions.{key}");
        if missing.is_empty() && extra.is_empty() {
            findings.push(f(Level::Ok, s, format!("{} entries", want.len())));
        } else {
            findings.push(f(
                Level::Drift,
                s,
                format!(
                    "{} missing, {} not in permissions.toml: {:?}",
                    missing.len(),
                    extra.len(),
                    extra.iter().take(5).collect::<Vec<_>>()
                ),
            ));
        }
    }
    // Claude settings.local.json: never written; its extras are unpromoted approvals.
    let local = p.claude.join("settings.local.json");
    if let Ok(v) = read_json(&local) {
        let want = render::render_perms(&src.perms.allow, Tool::Claude);
        let want_set: std::collections::HashSet<&str> = want.iter().map(String::as_str).collect();
        for e in str_list(v.pointer("/permissions/allow")) {
            if !want_set.contains(e.as_str()) {
                unpromoted.push((Tool::Claude, local.clone(), e));
            }
        }
    }
    // AGY settings.json: co-owned (AGY appends approvals). Desired ⊆ current; extras unpromoted.
    let ag = p.agy_settings();
    let av = match read_json_opt(&ag)? {
        Some(v) => v,
        None => {
            findings.push(missing_file(p, &ag));
            json!({})
        }
    };
    for (key, set) in [("allow", &src.perms.allow), ("deny", &src.perms.deny)] {
        let want = render::render_perms(set, Tool::Agy);
        let have = str_list(av.pointer(&format!("/permissions/{key}")));
        let want_set: std::collections::HashSet<&str> = want.iter().map(String::as_str).collect();
        let have_set: std::collections::HashSet<&str> = have.iter().map(String::as_str).collect();
        let missing: Vec<_> = want
            .iter()
            .filter(|w| !have_set.contains(w.as_str()))
            .collect();
        let s = format!("agy settings.json permissions.{key}");
        if missing.is_empty() {
            findings.push(f(Level::Ok, s, format!("{} entries", want.len())));
        } else {
            findings.push(f(
                Level::Drift,
                s,
                format!(
                    "{} missing: {:?}",
                    missing.len(),
                    missing.iter().take(5).collect::<Vec<_>>()
                ),
            ));
        }
        if key == "allow" {
            let ag_path = ag.clone();
            for e in have.into_iter().filter(|h| !want_set.contains(h.as_str())) {
                unpromoted.push((Tool::Agy, ag_path.clone(), e));
            }
        } else if let Some(e) = have.iter().find(|h| !want_set.contains(h.as_str())) {
            findings.push(f(
                Level::Drift,
                "agy settings.json permissions.deny",
                format!("'{e}' is not in permissions.toml (deny is fully owned)"),
            ));
        }
    }
    if !unpromoted.is_empty() {
        findings.push(f(
            Level::Unpromoted,
            "permissions",
            format!(
                "{} tool-appended approvals not in permissions.toml (agent-sync promote --list)",
                unpromoted.len()
            ),
        ));
    }
    Ok(PermState {
        findings,
        unpromoted,
    })
}

fn merge_agy_perms(av: &mut Value, src: &Sources) -> Result<()> {
    let perms = av
        .as_object_mut()
        .context("agy settings.json is not an object")?
        .entry("permissions")
        .or_insert_with(|| json!({}));
    for (key, set) in [("allow", &src.perms.allow), ("deny", &src.perms.deny)] {
        let want = render::render_perms(set, Tool::Agy);
        let mut merged = want.clone();
        if key == "allow" {
            // Preserve tool-appended approvals (unpromoted) after the owned block.
            for e in str_list(perms.get(key)) {
                if !merged.contains(&e) {
                    merged.push(e);
                }
            }
        }
        perms[key] = json!(merged);
    }
    Ok(())
}

pub fn apply_perms(p: &Paths, src: &Sources, force: bool, log: &crate::out::Log) -> Result<()> {
    // Claude: fully owned keys only; everything else in settings.json is untouched.
    let path = p.claude.join("settings.json");
    let existing = read_json_opt(&path)?;
    let exists = existing.is_some();
    let mut cs = existing.unwrap_or_else(|| json!({}));
    let before = cs.clone();
    {
        let perms = cs
            .as_object_mut()
            .context("settings.json is not an object")?
            .entry("permissions")
            .or_insert_with(|| json!({}));
        perms["allow"] = json!(render::render_perms(&src.perms.allow, Tool::Claude));
        perms["deny"] = json!(render::render_perms(&src.perms.deny, Tool::Claude));
    }
    if cs != before || !exists {
        atomic_write(&path, &pretty(&cs)?)?;
        log.info("wrote claude settings.json permissions");
    }
    // AGY: merge, refuse while AGY runs (it rewrites this file on interactive approvals).
    let ag = p.agy_settings();
    for attempt in 0..2 {
        let existing = read_json_opt(&ag)?;
        let exists = existing.is_some();
        let mut av = existing.unwrap_or_else(|| json!({}));
        let before = av.clone();
        merge_agy_perms(&mut av, src)?;
        if av == before && exists {
            return Ok(());
        }
        if agy_running() && !force {
            return Err(AgyRunning(ag.display().to_string()).into());
        }
        let text = pretty(&av)?;
        atomic_write(&ag, &text)?;
        // Verify it survived: AGY may have written between our read and rename. Ceiling: this
        // is a check-then-write race, accepted because AGY only writes on an interactive
        // approval; upgrade path is an advisory flock if the retry below ever fires.
        let mut again = read_json(&ag)?;
        let snapshot = again.clone();
        merge_agy_perms(&mut again, src)?;
        if again == snapshot {
            log.info("wrote agy settings.json permissions");
            return Ok(());
        }
        log.warn(&format!(
            "agy settings.json changed during write (attempt {attempt}); re-merging"
        ));
    }
    bail!("agy settings.json kept changing during apply; not retrying further")
}

/// `promote --add`: move native entries into permissions.toml (neutral slot inferred).
pub fn promote_add(p: &Paths, entries: &[String]) -> Result<()> {
    let path = p.sync().join("permissions.toml");
    let mut pf: crate::model::PermFile = crate::model::read_toml(&path)?;
    for e in entries {
        let tool =
            if e.starts_with("command(") || e.starts_with("read_file(") || e.starts_with("mcp(") {
                Tool::Agy
            } else {
                Tool::Claude
            };
        let (slot, v) = render::parse_perm(e, tool);
        let list = match slot {
            "shell" => &mut pf.allow.shell,
            "shell_glob" => &mut pf.allow.shell_glob,
            "shell_exact" => &mut pf.allow.shell_exact,
            "read" => &mut pf.allow.read,
            "mcp" => &mut pf.allow.mcp,
            "agy" => &mut pf.allow.agy,
            _ => &mut pf.allow.claude,
        };
        if !list.contains(&v) {
            list.push(v);
        }
    }
    atomic_write(&path, &crate::import::perm_toml(&pf)?)
}

/// `promote --drop`: remove approvals from the tool files that hold them (never from the source).
pub fn promote_drop(p: &Paths, entries: &[String], force: bool) -> Result<usize> {
    let mut n = 0;
    for (path, is_agy) in [
        (p.agy_settings(), true),
        (p.claude.join("settings.local.json"), false),
    ] {
        let Ok(mut v) = read_json(&path) else {
            continue;
        };
        let have = str_list(v.pointer("/permissions/allow"));
        let kept: Vec<_> = have
            .iter()
            .filter(|e| !entries.contains(e))
            .cloned()
            .collect();
        if kept.len() != have.len() {
            if is_agy && agy_running() && !force {
                return Err(AgyRunning(path.display().to_string()).into());
            }
            n += have.len() - kept.len();
            v["permissions"]["allow"] = json!(kept);
            atomic_write(&path, &pretty(&v)?)?;
        }
    }
    Ok(n)
}

// ---------- hooks ----------

pub fn check_hooks(p: &Paths, src: &Sources) -> Result<Vec<Finding>> {
    let mut out = Vec::new();
    let want = render::render_claude_hooks(&src.hooks);
    // A missing settings.json is already reported by check_perms.
    let cs = read_json_opt(&p.claude.join("settings.json"))?.unwrap_or_else(|| json!({}));
    let have = cs.get("hooks").cloned().unwrap_or(json!({}));
    out.push(if have == want {
        f(Level::Ok, "claude settings.json hooks", "")
    } else {
        f(
            Level::Drift,
            "claude settings.json hooks",
            "differ from hooks.toml",
        )
    });
    let want = render::render_agy_hooks(&src.hooks);
    let path = p.gemini.join("config/hooks.json");
    let have = read_json(&path).unwrap_or(json!({}));
    out.push(if have == want {
        f(
            Level::Ok,
            "agy hooks.json",
            format!("{} hooks", want.as_object().map_or(0, Map::len)),
        )
    } else {
        f(Level::Drift, "agy hooks.json", "differs from hooks.toml")
    });
    let want = render::render_codex_hooks(&src.hooks);
    let path = p.codex.join("hooks.json");
    let have = read_json(&path).unwrap_or(json!({}));
    out.push(if have == want {
        f(Level::Ok, "codex hooks.json", "")
    } else {
        f(Level::Drift, "codex hooks.json", "differs from hooks.toml")
    });
    Ok(out)
}

pub fn apply_hooks(p: &Paths, src: &Sources, log: &crate::out::Log) -> Result<()> {
    let path = p.claude.join("settings.json");
    let mut cs = read_json_opt(&path)?.unwrap_or_else(|| json!({}));
    let want = render::render_claude_hooks(&src.hooks);
    if cs.get("hooks") != Some(&want) {
        cs.as_object_mut()
            .context("settings.json is not an object")?
            .insert("hooks".into(), want);
        atomic_write(&path, &pretty(&cs)?)?;
        log.info("wrote claude settings.json hooks");
    }
    let want = render::render_agy_hooks(&src.hooks);
    let hp = p.gemini.join("config/hooks.json");
    let have = read_json(&hp).ok();
    let empty = want.as_object().is_some_and(Map::is_empty);
    if have.as_ref() != Some(&want) && !(empty && have.is_none()) {
        atomic_write(&hp, &pretty(&want)?)?;
        log.info("wrote agy hooks.json");
    }
    let want = render::render_codex_hooks(&src.hooks);
    let hp = p.codex.join("hooks.json");
    let have = read_json(&hp).ok();
    if have.as_ref() != Some(&want) {
        atomic_write(&hp, &pretty(&want)?)?;
        log.info("wrote codex hooks.json");
    }
    Ok(())
}

// ---------- model tags ----------

/// No `models.toml` means no projection (and no finding).
pub fn check_model_tags(p: &Paths, src: &Sources) -> Result<Vec<Finding>> {
    let Some(models) = &src.models else {
        return Ok(Vec::new());
    };
    let want = models.render()?;
    let path = p.model_tags();
    let s = "agent-terminal model-tags.json";
    Ok(vec![match std::fs::read_to_string(&path) {
        Ok(have) if have == want => f(Level::Ok, s, format!("{} models", models.models.len())),
        Ok(_) => f(Level::Drift, s, "differs from models.toml"),
        Err(_) => f(Level::Drift, s, "missing"),
    }])
}

pub fn apply_model_tags(p: &Paths, src: &Sources, log: &crate::out::Log) -> Result<()> {
    let Some(models) = &src.models else {
        return Ok(());
    };
    let want = models.render()?;
    let path = p.model_tags();
    if std::fs::read_to_string(&path).ok().as_deref() != Some(want.as_str()) {
        atomic_write(&path, &want)?;
        log.info("wrote agent-terminal model-tags.json");
    }
    Ok(())
}

// ---------- private repo overlays ----------

/// Repos with a private `.claude/AGENTS.md` overlay need `.agents/AGENTS.md` → it, so AGY sees it too.
fn overlay_repos(p: &Paths) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&p.git_root) {
        for e in rd.flatten() {
            if e.path().join(".claude/AGENTS.md").is_file() {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out
}

const OVERLAY_TARGET: &str = "../.claude/AGENTS.md";

pub fn check_overlays(p: &Paths) -> Vec<Finding> {
    overlay_repos(p)
        .into_iter()
        .map(|r| {
            let link = r.join(".agents/AGENTS.md");
            let s = format!("overlay twin {}", link.display());
            match std::fs::read_link(&link) {
                Ok(t) if t == Path::new(OVERLAY_TARGET) => f(Level::Ok, s, ""),
                _ => f(
                    Level::Drift,
                    s,
                    "missing (AGY cannot see the private .claude/AGENTS.md)",
                ),
            }
        })
        .collect()
}

pub fn apply_overlays(p: &Paths, log: &crate::out::Log) -> Result<()> {
    for r in overlay_repos(p) {
        let link = r.join(".agents/AGENTS.md");
        if std::fs::read_link(&link).ok().as_deref() == Some(Path::new(OVERLAY_TARGET)) {
            continue;
        }
        if link.exists() {
            bail!(
                "{} exists and is not the overlay link; resolve by hand",
                link.display()
            );
        }
        crate::fsx::replace_link(&link, Path::new(OVERLAY_TARGET))?;
        // Keep the private link out of the (possibly public) repo without touching .gitignore.
        let exclude = r.join(".git/info/exclude");
        if exclude.parent().is_some_and(Path::is_dir) {
            let cur = std::fs::read_to_string(&exclude).unwrap_or_default();
            if !cur.lines().any(|l| l.trim() == "/.agents/AGENTS.md") {
                let sep = if cur.is_empty() || cur.ends_with('\n') {
                    ""
                } else {
                    "\n"
                };
                atomic_write(&exclude, &format!("{cur}{sep}/.agents/AGENTS.md\n"))?;
            }
        }
        log.info(&format!("linked overlay twin {}", link.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("agent-sync-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // A temporary HOME is the only place these tests may write.
        crate::fsx::allow_writes_under([d.clone()]);
        d
    }

    #[test]
    fn links_check_then_apply_converges() {
        let home = tmp("links");
        let p = Paths::from_home(&home, None);
        std::fs::create_dir_all(p.ac.join("rules")).unwrap();
        std::fs::write(p.ac.join("rules/core.md"), "core").unwrap();
        std::fs::write(p.ac.join("rules/codex.md"), "codex").unwrap();
        // No plugin manifest: reported, and apply refuses rather than guess a link name.
        assert!(check_links(&p).iter().any(|x| x.surface == "claude plugin"));
        let log = crate::out::Log::stderr_only("t");
        assert!(apply_links(&p, &log).is_err());
        std::fs::create_dir_all(p.ac.join(".claude-plugin")).unwrap();
        std::fs::write(
            p.ac.join(".claude-plugin/plugin.json"),
            r#"{"name": "my-skills"}"#,
        )
        .unwrap();
        assert!(check_links(&p).iter().any(|x| x.level == Level::Drift));
        apply_links(&p, &log).unwrap();
        assert_eq!(
            std::fs::read_link(p.claude.join("plugins/my-skills")).unwrap(),
            p.ac
        );
        assert!(
            check_links(&p).iter().all(|x| x.level == Level::Ok),
            "{:?}",
            check_links(&p)
        );
        // A real directory in a link's place is refused, never clobbered.
        std::fs::remove_file(p.gemini.join("config/skills")).unwrap();
        std::fs::create_dir_all(p.gemini.join("config/skills")).unwrap();
        assert!(apply_links(&p, &log).is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn atomic_write_keeps_backup_and_mode_and_refuses_secrets() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("aw");
        let fpath = d.join("s.json");
        std::fs::write(&fpath, "old").unwrap();
        std::fs::set_permissions(&fpath, std::fs::Permissions::from_mode(0o600)).unwrap();
        atomic_write(&fpath, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&fpath).unwrap(), "new");
        assert_eq!(
            std::fs::read_to_string(d.join("s.json.agent-sync.bak")).unwrap(),
            "old"
        );
        assert_eq!(
            std::fs::metadata(&fpath).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let token = format!("ghp_{}", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8");
        assert!(atomic_write(&fpath, &token).is_err());
        assert_eq!(std::fs::read_to_string(&fpath).unwrap(), "new");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn model_tags_check_apply_converges_and_absent_source_is_silent() {
        let home = tmp("tags");
        let p = Paths::from_home(&home, None);
        let log = crate::out::Log::stderr_only("t");
        let mut src = Sources {
            map: Default::default(),
            agents: vec![],
            mcp: Default::default(),
            perms: Default::default(),
            hooks: Default::default(),
            models: None,
        };
        assert!(check_model_tags(&p, &src).unwrap().is_empty());
        apply_model_tags(&p, &src, &log).unwrap();
        assert!(!p.model_tags().exists());

        src.models = Some(
            toml::from_str(
                "[vocabulary]\ncode = \"c\"\n[[model]]\nmatch = \"*a*\"\ntier = \"fast\"\ntags = [\"code\"]\n",
            )
            .unwrap(),
        );
        assert_eq!(check_model_tags(&p, &src).unwrap()[0].level, Level::Drift);
        apply_model_tags(&p, &src, &log).unwrap();
        assert_eq!(check_model_tags(&p, &src).unwrap()[0].level, Level::Ok);
        std::fs::write(p.model_tags(), "{}").unwrap();
        assert_eq!(check_model_tags(&p, &src).unwrap()[0].level, Level::Drift);
        apply_model_tags(&p, &src, &log).unwrap();
        assert!(p
            .model_tags()
            .with_extension("json.agent-sync.bak")
            .exists());
        assert_eq!(check_model_tags(&p, &src).unwrap()[0].level, Level::Ok);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn xdg_config_home_overrides_only_when_absolute() {
        let home = tmp("xdg");
        let base = Paths::from_home(&home, None);
        assert_eq!(
            base.model_tags(),
            home.join(".config/agent-terminal/model-tags.json")
        );
        let p = Paths::from_home(&home, None).with_xdg_config(Some("/x/cfg".into()));
        assert_eq!(
            p.model_tags(),
            PathBuf::from("/x/cfg/agent-terminal/model-tags.json")
        );
        let p = Paths::from_home(&home, None).with_xdg_config(Some("rel".into()));
        assert_eq!(p.config, home.join(".config"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn agy_perm_merge_keeps_unpromoted_approvals() {
        let src_perm: crate::model::PermFile =
            toml::from_str("[allow]\nshell = [\"git status\"]\n[deny]\nshell_exact = [\"env\"]\n")
                .unwrap();
        let src = Sources {
            map: Default::default(),
            agents: vec![],
            mcp: Default::default(),
            perms: src_perm,
            hooks: Default::default(),
            models: None,
        };
        let mut av = json!({"model": "x", "permissions": {"allow": ["command(ps)"], "deny": ["command(old)"]}});
        merge_agy_perms(&mut av, &src).unwrap();
        assert_eq!(
            av["permissions"]["allow"],
            json!(["command(git status)", "command(ps)"])
        );
        assert_eq!(av["permissions"]["deny"], json!(["command(env)"]));
        assert_eq!(av["model"], "x");
    }

    #[test]
    fn codex_server_replace_keeps_tool_approvals_and_finds_extras() {
        let mut doc: DocumentMut = "[mcp_servers.pw]\ncommand = \"npx\"\nargs = [\"old\"]\n\n\
             [mcp_servers.pw.tools.nav]\napproval_mode = \"approve\"\n\n\
             [mcp_servers.gone]\ncommand = \"x\"\n"
            .parse()
            .unwrap();
        let want_server = json!({"command": "npx", "args": ["new"]});
        let mut want = Map::new();
        want.insert("pw".into(), want_server.clone());
        let servers = doc["mcp_servers"].as_table_mut().unwrap();
        assert_eq!(codex_extra_servers(Some(servers), &want), vec!["gone"]);
        let item = codex_server_item(&want_server).unwrap();
        replace_codex_server(servers, "pw", item);
        let out = doc.to_string();
        assert!(out.contains("args = [\"new\"]"), "{out}");
        assert!(out.contains("[mcp_servers.pw.tools.nav]"), "{out}");
        // check_mcp compares definitions only: a kept tools table must not read as drift.
        let expected = codex_server_item(&want_server).unwrap();
        assert_eq!(doc["mcp_servers"]["pw"].to_string(), expected.to_string());
    }
}
