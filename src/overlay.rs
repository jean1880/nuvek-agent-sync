//! Overlay layering: a private overlay checkout over a public base checkout.
//!
//! The two layers are resolved into one **resolved source tree**: a directory of symlinks to the
//! winning file of each relative path, plus real files for the merged TOML sources. Every link
//! agent-sync writes then points into that tree, so the tools see one source.
//!
//! - **Roots.** Only [`ROOTS`] are resolved: the plugin manifest, rules, skills and the neutral
//!   `sync/` sources.
//! - **Paths.** The overlay wins path by path. A skill (`skills/<name>/`) is one unit: the whole
//!   directory comes from one layer, so a base skill's stray files never leak into the overlay's
//!   version of it. An overlay's `overlay.toml` can drop base paths: `remove = ["skills/x"]`.
//! - **Merged sources.** When the overlay has one of [`MERGED`], it is merged with the base's
//!   copy by key (see each `merge_*`), and `[remove]` in the overlay deletes base entries. When
//!   only the base has it, it is a plain link like any other file.
//! - **Atomic.** `<state>/resolved` is a symlink to `<state>/generations/<id>`. A rebuild writes a
//!   new generation and renames a fresh symlink over `resolved`; a failed build changes nothing.
//!
//! With no overlay configured none of this runs and every path is the base checkout.

use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use toml::{Table, Value};

/// The source roots that are resolved. Everything else in a checkout is ignored.
pub const ROOTS: [&str; 4] = [".claude-plugin", "rules", "skills", "sync"];

/// Neutral sources merged by key when the overlay has them.
pub const MERGED: [&str; 5] = [
    "sync/map.toml",
    "sync/mcp.toml",
    "sync/hooks.toml",
    "sync/permissions.toml",
    "sync/models.toml",
];

/// The overlay's own control file (at its root, never resolved).
pub const OVERLAY_FILE: &str = "overlay.toml";

/// The two checkouts, as absolute paths.
#[derive(Debug, Clone)]
pub struct Layers {
    pub base: PathBuf,
    pub overlay: PathBuf,
}

/// Where the resolved tree lives: `$XDG_STATE_HOME/agent-sync` (absolute only), else
/// `~/.local/state/agent-sync`.
pub fn state_dir(home: &Path, xdg_state: Option<String>) -> PathBuf {
    match xdg_state.filter(|x| Path::new(x).is_absolute()) {
        Some(x) => PathBuf::from(x).join("agent-sync"),
        None => home.join(".local/state/agent-sync"),
    }
}

/// The stable path every link points into.
pub fn resolved_path(state: &Path) -> PathBuf {
    state.join("resolved")
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Item {
    /// A symlink to this file or directory in one layer.
    Link(PathBuf),
    /// A merged source, written as a real file.
    Merged(String),
}

#[derive(serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct OverlayFile {
    #[serde(default)]
    remove: Vec<String>,
}

/// A relative path that stays inside the checkout.
fn safe_rel(s: &str) -> Result<PathBuf> {
    let p = PathBuf::from(s.trim_end_matches('/'));
    let ok = !s.is_empty()
        && p.components().all(|c| matches!(c, Component::Normal(_)))
        && p.components().next().is_some();
    if !ok {
        bail!("{OVERLAY_FILE}: remove entry '{s}' must be a relative path inside the checkout");
    }
    Ok(p)
}

fn removed_paths(overlay: &Path) -> Result<Vec<PathBuf>> {
    let path = overlay.join(OVERLAY_FILE);
    let text = match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        r => r.with_context(|| format!("reading {}", path.display()))?,
    };
    let f: OverlayFile =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    f.remove.iter().map(|s| safe_rel(s)).collect()
}

/// The resolved tree being planned: links and merged files, and the directories between them.
#[derive(Default)]
struct Plan {
    items: BTreeMap<PathBuf, Item>,
    dirs: BTreeSet<PathBuf>,
}

impl Plan {
    /// Insert `rel`, dropping whatever it shadows: anything at or under it, and a file at an
    /// ancestor.
    fn insert(&mut self, rel: PathBuf, item: Item) {
        self.items.retain(|k, _| !k.starts_with(&rel));
        self.dirs.retain(|k| !k.starts_with(&rel));
        for a in rel.ancestors().skip(1) {
            self.items.remove(a);
        }
        self.items.insert(rel, item);
    }
}

/// `None` when nothing is there; any other failure is an error, so an unreadable item can never
/// silently drop out of the tree (and have its links pruned).
fn stat(path: &Path, follow: bool) -> Result<Option<std::fs::Metadata>> {
    let r = if follow {
        std::fs::metadata(path)
    } else {
        std::fs::symlink_metadata(path)
    };
    match r {
        Ok(m) => Ok(Some(m)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn walk(layer: &Path, rel: &Path, removed: &[PathBuf], plan: &mut Plan) -> Result<()> {
    if removed.iter().any(|r| rel.starts_with(r)) {
        return Ok(());
    }
    let abs = layer.join(rel);
    // A root (`rules`, `skills`, ...) may itself be a link and is followed. Below it a link is a
    // leaf, linked as it is: never walked, so a link cycle or a link to `/` cannot be expanded.
    let is_root = rel.components().count() == 1;
    let Some(meta) = stat(&abs, is_root)? else {
        return Ok(());
    };
    // A skill is one unit: the whole directory comes from one layer.
    let is_skill = rel.parent() == Some(Path::new("skills"));
    if meta.file_type().is_symlink() || meta.is_file() || (is_skill && meta.is_dir()) {
        plan.insert(rel.to_path_buf(), Item::Link(abs));
        return Ok(());
    }
    if !meta.is_dir() {
        return Ok(());
    }
    plan.dirs.insert(rel.to_path_buf());
    let mut entries = Vec::new();
    for e in std::fs::read_dir(&abs).with_context(|| format!("reading {}", abs.display()))? {
        let name = e
            .with_context(|| format!("reading {}", abs.display()))?
            .file_name();
        if name != ".git" {
            entries.push(name);
        }
    }
    entries.sort();
    for name in entries {
        walk(layer, &rel.join(name), removed, plan)?;
    }
    Ok(())
}

/// Take the overlay's `[remove]` table, rejecting keys this file does not support.
fn take_remove(over: &mut Table, file: &str, allowed: &[&str]) -> Result<Table> {
    let Some(v) = over.remove("remove") else {
        return Ok(Table::new());
    };
    let Value::Table(t) = v else {
        bail!("{file}: [remove] must be a table");
    };
    for k in t.keys() {
        if !allowed.contains(&k.as_str()) {
            bail!("{file}: [remove] has no key '{k}' (allowed: {allowed:?})");
        }
    }
    Ok(t)
}

fn names(t: &Table, key: &str, file: &str) -> Result<Vec<String>> {
    match t.get(key) {
        None => Ok(Vec::new()),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .map(String::from)
                    .with_context(|| format!("{file}: [remove] {key} must list strings"))
            })
            .collect(),
        Some(_) => bail!("{file}: [remove] {key} must be an array"),
    }
}

fn table_mut<'a>(t: &'a mut Table, key: &str, file: &str) -> Result<&'a mut Table> {
    match t
        .entry(key.to_string())
        .or_insert_with(|| Value::Table(Table::new()))
    {
        Value::Table(x) => Ok(x),
        _ => bail!("{file}: '{key}' must be a table"),
    }
}

fn array_mut<'a>(t: &'a mut Table, key: &str, file: &str) -> Result<&'a mut Vec<Value>> {
    match t
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
    {
        Value::Array(x) => Ok(x),
        _ => bail!("{file}: '{key}' must be an array"),
    }
}

/// Append the values of `add` not already in `into`, keeping order.
fn union(into: &mut Vec<Value>, add: Vec<Value>) {
    for v in add {
        if !into.contains(&v) {
            into.push(v);
        }
    }
}

/// `mcp.toml`: an overlay `[servers.<name>]` replaces the base entry whole (a field-by-field
/// merge could leave both a `command` and a `url`). `[remove] servers = [...]` deletes.
pub fn merge_mcp(mut base: Table, mut over: Table) -> Result<Table> {
    let f = "mcp.toml";
    let rm = take_remove(&mut over, f, &["servers"])?;
    let servers = table_mut(&mut base, "servers", f)?;
    for n in names(&rm, "servers", f)? {
        servers.remove(&n);
    }
    if let Some(Value::Table(add)) = over.remove("servers") {
        for (k, v) in add {
            servers.insert(k, v);
        }
    }
    for (k, v) in over {
        base.insert(k, v);
    }
    Ok(base)
}

/// `hooks.toml`: `[[hook]]` keyed by `name`. A same-name overlay hook replaces the base one in
/// place (event order is rendered order); new hooks follow. `[remove] hooks = [...]` deletes.
pub fn merge_hooks(mut base: Table, mut over: Table) -> Result<Table> {
    let f = "hooks.toml";
    let rm = names(&take_remove(&mut over, f, &["hooks"])?, "hooks", f)?;
    let name_of = |v: &Value| v.get("name").and_then(Value::as_str).map(String::from);
    let list = array_mut(&mut base, "hook", f)?;
    list.retain(|h| !name_of(h).is_some_and(|n| rm.contains(&n)));
    if let Some(Value::Array(add)) = over.remove("hook") {
        for h in add {
            let n = name_of(&h).with_context(|| format!("{f}: an overlay hook has no name"))?;
            match list.iter_mut().find(|x| name_of(x).as_deref() == Some(&n)) {
                Some(slot) => *slot = h,
                None => list.push(h),
            }
        }
    }
    for (k, v) in over {
        base.insert(k, v);
    }
    Ok(base)
}

/// `permissions.toml`: every slot list is a set, unioned (base order, then the overlay's new
/// entries). `[remove.allow]` / `[remove.deny]`, with the same slot keys, delete exact entries.
pub fn merge_perms(mut base: Table, mut over: Table) -> Result<Table> {
    let f = "permissions.toml";
    let rm = take_remove(&mut over, f, &["allow", "deny"])?;
    for dir in ["allow", "deny"] {
        let target = table_mut(&mut base, dir, f)?;
        if let Some(Value::Table(drop)) = rm.get(dir) {
            for (slot, v) in drop {
                let Value::Array(gone) = v else {
                    bail!("{f}: [remove.{dir}] {slot} must be an array");
                };
                if let Some(Value::Array(list)) = target.get_mut(slot) {
                    list.retain(|e| !gone.contains(e));
                }
            }
        } else if rm.contains_key(dir) {
            bail!("{f}: [remove.{dir}] must be a table");
        }
        if let Some(add) = over.remove(dir) {
            let Value::Table(add) = add else {
                bail!("{f}: [{dir}] must be a table");
            };
            for (slot, v) in add {
                let Value::Array(v) = v else {
                    bail!("{f}: [{dir}] {slot} must be an array");
                };
                union(array_mut(target, &slot, f)?, v);
            }
        }
    }
    for (k, v) in over {
        base.insert(k, v);
    }
    Ok(base)
}

/// `models.toml`: `[vocabulary]` merges by key, `[[model]]` is keyed by `match` and `driver`
/// (replaced in place, new ones appended), and `[effort]` is replaced whole.
/// `[remove] vocabulary = [...]` and `models = [<match>...]` delete.
pub fn merge_models(mut base: Table, mut over: Table) -> Result<Table> {
    let f = "models.toml";
    let rm = take_remove(&mut over, f, &["vocabulary", "models"])?;
    let vocab = table_mut(&mut base, "vocabulary", f)?;
    for n in names(&rm, "vocabulary", f)? {
        vocab.remove(&n);
    }
    if let Some(Value::Table(add)) = over.remove("vocabulary") {
        for (k, v) in add {
            vocab.insert(k, v);
        }
    }
    let gone = names(&rm, "models", f)?;
    let key = |v: &Value| {
        (
            v.get("match").and_then(Value::as_str).map(String::from),
            v.get("driver").and_then(Value::as_str).map(String::from),
        )
    };
    let list = array_mut(&mut base, "model", f)?;
    list.retain(|m| !key(m).0.is_some_and(|m| gone.contains(&m)));
    if let Some(Value::Array(add)) = over.remove("model") {
        for m in add {
            let k = key(&m);
            match list.iter_mut().find(|x| key(x) == k) {
                Some(slot) => *slot = m,
                None => list.push(m),
            }
        }
    }
    for (k, v) in over {
        base.insert(k, v);
    }
    Ok(base)
}

/// `map.toml`: tables merge recursively, arrays are unioned (so an overlay adds placeholder
/// names), and other values are replaced. It has no `[remove]`.
pub fn merge_map(base: Table, over: Table) -> Result<Table> {
    if over.contains_key("remove") {
        bail!("map.toml: [remove] is not supported; override the value instead");
    }
    fn deep(mut base: Table, over: Table) -> Table {
        for (k, v) in over {
            let merged = match (base.remove(&k), v) {
                (Some(Value::Table(b)), Value::Table(o)) => Value::Table(deep(b, o)),
                (Some(Value::Array(mut b)), Value::Array(o)) => {
                    union(&mut b, o);
                    Value::Array(b)
                }
                (_, o) => o,
            };
            base.insert(k, merged);
        }
        base
    }
    Ok(deep(base, over))
}

fn merge_file(rel: &str, base: Table, over: Table) -> Result<Table> {
    match rel {
        "sync/map.toml" => merge_map(base, over),
        "sync/mcp.toml" => merge_mcp(base, over),
        "sync/hooks.toml" => merge_hooks(base, over),
        "sync/permissions.toml" => merge_perms(base, over),
        "sync/models.toml" => merge_models(base, over),
        _ => bail!("{rel} is not a merged source"),
    }
}

/// Every item of the resolved tree, plus the directories it needs.
fn plan(layers: &Layers, label: &dyn Fn(&Path) -> String) -> Result<Plan> {
    let removed = removed_paths(&layers.overlay)?;
    let mut plan = Plan::default();
    for root in ROOTS {
        walk(&layers.base, Path::new(root), &removed, &mut plan)?;
    }
    for root in ROOTS {
        walk(&layers.overlay, Path::new(root), &[], &mut plan)?;
    }
    for rel in MERGED {
        let over_path = layers.overlay.join(rel);
        if !over_path.is_file() {
            continue;
        }
        let base_path = layers.base.join(rel);
        let base_removed = removed.iter().any(|r| Path::new(rel).starts_with(r));
        let base_used = base_path.is_file() && !base_removed;
        let base = if base_used {
            crate::model::read_toml(&base_path)?
        } else {
            Table::new()
        };
        let merged = merge_file(rel, base, crate::model::read_toml(&over_path)?)
            .with_context(|| format!("merging {}", over_path.display()))?;
        let from = if base_used {
            format!("{} merged with {}", label(&base_path), label(&over_path))
        } else {
            label(&over_path)
        };
        let text = format!(
            "# {}{from}. Edit that source, not this file.\n{}",
            crate::render::GENERATED_MARK,
            toml::to_string_pretty(&merged)?
        );
        plan.insert(PathBuf::from(rel), Item::Merged(text));
    }
    Ok(plan)
}

/// Write the resolved tree into `dest`, which must not exist yet.
pub fn build(layers: &Layers, dest: &Path, label: &dyn Fn(&Path) -> String) -> Result<()> {
    create_private_dir(dest)?;
    fill(layers, dest, label)
}

/// Private, and never an existing directory: a path someone else prepared is refused.
fn create_private_dir(dest: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(dest)
        .with_context(|| format!("creating {}", dest.display()))
}

/// Write the resolved tree into `dest`, a directory this run just created.
fn fill(layers: &Layers, dest: &Path, label: &dyn Fn(&Path) -> String) -> Result<()> {
    let Plan { items, dirs } = plan(layers, label)?;
    for d in &dirs {
        std::fs::create_dir_all(dest.join(d))?;
    }
    for (rel, item) in &items {
        let at = dest.join(rel);
        if let Some(parent) = at.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match item {
            Item::Link(target) => std::os::unix::fs::symlink(target, &at)
                .with_context(|| format!("linking {}", at.display()))?,
            Item::Merged(text) => {
                crate::sync::assert_clean(&at.display().to_string(), text)?;
                std::fs::write(&at, text)?;
            }
        }
    }
    // Written last: only a complete build carries it, and only a marked directory is ever pruned.
    std::fs::write(dest.join(MARKER), MARKER_TEXT)?;
    Ok(())
}

/// Marks a directory as a resolved tree agent-sync built.
const MARKER: &str = ".agent-sync-generation";
const MARKER_TEXT: &str = "resolved source tree built by agent-sync\n";

/// A generation agent-sync built: a real directory holding the marker as a regular file.
fn is_ours(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir())
        && std::fs::symlink_metadata(dir.join(MARKER)).is_ok_and(|m| m.is_file())
        && std::fs::read_to_string(dir.join(MARKER)).is_ok_and(|t| t == MARKER_TEXT)
}

/// One entry of a tree, for comparing two builds.
#[derive(Debug, PartialEq, Eq)]
enum Entry {
    Dir,
    Link(PathBuf),
    File(Vec<u8>),
}

fn signature(root: &Path) -> Result<BTreeMap<PathBuf, Entry>> {
    fn go(root: &Path, rel: &Path, out: &mut BTreeMap<PathBuf, Entry>) -> Result<()> {
        for e in std::fs::read_dir(root.join(rel))? {
            let r = rel.join(e?.file_name());
            let p = root.join(&r);
            let ft = std::fs::symlink_metadata(&p)?.file_type();
            if ft.is_symlink() {
                out.insert(r, Entry::Link(std::fs::read_link(&p)?));
            } else if ft.is_dir() {
                out.insert(r.clone(), Entry::Dir);
                go(root, &r, out)?;
            } else {
                out.insert(r, Entry::File(std::fs::read(&p)?));
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    go(root, Path::new(""), &mut out)?;
    Ok(out)
}

/// True when the tree at `a` has exactly the entries of the tree at `b`.
pub fn same_tree(a: &Path, b: &Path) -> bool {
    match (signature(a), signature(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// A scratch build for `check`, removed on drop.
pub struct Scratch(pub PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

use crate::fsx::unique;

/// Resolve into a private temporary directory: `check` reads this, and never writes the state.
pub fn build_scratch(layers: &Layers, label: &dyn Fn(&Path) -> String) -> Result<Scratch> {
    let dir = std::env::temp_dir().join(unique("agent-sync-resolve"));
    // Owned (and so removed on drop) only once this run has created it.
    create_private_dir(&dir)?;
    let scratch = Scratch(dir.clone());
    fill(layers, &dir, label)?;
    Ok(scratch)
}

/// The live generation's name: `resolved` must be absent, or a link to `generations/<name>`.
/// Anything else (a directory, a link elsewhere) was not made by agent-sync and is refused.
fn live_generation(state: &Path) -> Result<Option<std::ffi::OsString>> {
    let resolved = resolved_path(state);
    let Ok(meta) = std::fs::symlink_metadata(&resolved) else {
        return Ok(None);
    };
    let target = if meta.file_type().is_symlink() {
        std::fs::read_link(&resolved)?
    } else {
        bail!(
            "{} is not a link; move it aside by hand first",
            resolved.display()
        );
    };
    let mut parts = target.components();
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Component::Normal(g)), Some(Component::Normal(name)), None) if g == "generations" => {
            Ok(Some(name.to_os_string()))
        }
        _ => bail!(
            "{} points at {}, not at a generation agent-sync built; move it aside by hand",
            resolved.display(),
            target.display()
        ),
    }
}

/// Rebuild `<state>/resolved` atomically. The caller holds the state lock for the whole run.
///
/// The candidate is built, then passed to `validate` (which loads and renders every projection
/// from it), and only then swapped in by one rename. The live generation and the one before it
/// are kept (the previous one for a manual rollback); other generations agent-sync marked as
/// its own are removed. Returns whether the tree changed.
pub fn install(
    layers: &Layers,
    state: &Path,
    label: &dyn Fn(&Path) -> String,
    validate: &dyn Fn(&Path) -> Result<()>,
    log: &crate::out::Log,
) -> Result<bool> {
    crate::fsx::refuse_symlink(state)?;
    let gens = state.join("generations");
    crate::fsx::refuse_symlink(&gens)?;
    std::fs::create_dir_all(&gens)?;
    let before = live_generation(state)?;
    let id = unique("g");
    let fresh = gens.join(&id);
    let changed = match swap_in(layers, state, &id, label, validate) {
        Ok(changed) => changed,
        Err(e) => {
            // The old tree is untouched; drop the half-built one.
            let _ = std::fs::remove_dir_all(&fresh);
            return Err(e);
        }
    };
    if !changed {
        let _ = std::fs::remove_dir_all(&fresh);
    }
    let live = live_generation(state)?;
    // Keep the live generation and the one before it. On an unchanged run the one before it
    // is the newest other marked generation.
    let mut keep: Vec<std::ffi::OsString> = live.iter().chain(before.iter()).cloned().collect();
    if !changed {
        let newest = std::fs::read_dir(&gens)?
            .flatten()
            .filter(|e| Some(e.file_name()) != live && is_ours(&e.path()))
            .max_by_key(|e| {
                std::fs::metadata(e.path().join(MARKER))
                    .and_then(|m| m.modified())
                    .ok()
            })
            .map(|e| e.file_name());
        keep.extend(newest);
    }
    for e in std::fs::read_dir(&gens)? {
        let e = e?;
        if !keep.contains(&e.file_name()) && is_ours(&e.path()) {
            // The new tree is already live: a cleanup failure must not stop the apply.
            if let Err(err) = std::fs::remove_dir_all(e.path()) {
                log.warn(&format!(
                    "could not remove old generation {}: {err}",
                    e.path().display()
                ));
            }
        }
    }
    if changed {
        log.info(&format!(
            "resolved source tree rebuilt at {}",
            resolved_path(state).display()
        ));
    }
    Ok(changed)
}

/// Build generation `id`, validate it, and rename a link to it over `resolved`. `false` (and
/// nothing swapped) when the installed tree already matches.
fn swap_in(
    layers: &Layers,
    state: &Path,
    id: &str,
    label: &dyn Fn(&Path) -> String,
    validate: &dyn Fn(&Path) -> Result<()>,
) -> Result<bool> {
    let fresh = state.join("generations").join(id);
    build(layers, &fresh, label).context("building the resolved source tree")?;
    validate(&fresh).context("the merged sources do not render; the live tree is unchanged")?;
    let resolved = resolved_path(state);
    if resolved.exists() && same_tree(&resolved, &fresh) {
        return Ok(false);
    }
    // Relative, so the state directory can move without breaking the link.
    crate::fsx::replace_link(&resolved, &Path::new("generations").join(id))
        .with_context(|| format!("swapping in {}", resolved.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Table {
        toml::from_str(s).unwrap()
    }

    #[test]
    fn mcp_overlay_replaces_adds_and_removes() {
        let base = t("[servers.a]\ncommand = \"a\"\n[servers.b]\ncommand = \"b\"\nargs = [\"x\"]\n[servers.c]\ncommand = \"c\"\n");
        let over = t("[remove]\nservers = [\"c\", \"absent\"]\n[servers.b]\nurl = \"http://localhost:1/mcp\"\n[servers.d]\ncommand = \"d\"\n");
        let m = merge_mcp(base, over).unwrap();
        let s = m["servers"].as_table().unwrap();
        assert_eq!(s.keys().collect::<Vec<_>>(), ["a", "b", "d"]);
        // Replaced whole: the base's command and args are gone.
        assert!(s["b"].get("command").is_none() && s["b"].get("args").is_none());
        assert!(m.get("remove").is_none());
        assert!(merge_mcp(Table::new(), t("[remove]\nbogus = []\n")).is_err());
    }

    #[test]
    fn hooks_replace_in_place_and_append() {
        let base = t("[[hook]]\nname = \"a\"\nevent = \"E\"\ncommand = \"1\"\n[[hook]]\nname = \"b\"\nevent = \"E\"\ncommand = \"2\"\n[[hook]]\nname = \"c\"\nevent = \"E\"\ncommand = \"3\"\n");
        let over = t("[remove]\nhooks = [\"c\"]\n[[hook]]\nname = \"a\"\nevent = \"E\"\ncommand = \"9\"\n[[hook]]\nname = \"z\"\nevent = \"E\"\ncommand = \"0\"\n");
        let m = merge_hooks(base, over).unwrap();
        let cmds: Vec<_> = m["hook"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["command"].as_str().unwrap())
            .collect();
        assert_eq!(cmds, ["9", "2", "0"]);
    }

    #[test]
    fn perms_union_and_exact_removal() {
        let base = t("[allow]\nshell = [\"ls\", \"cat\"]\n[deny]\nshell_exact = [\"env\"]\n");
        let over = t("[allow]\nshell = [\"cat\", \"git status\"]\nread = [\"/srv/x\"]\n[remove.allow]\nshell = [\"ls\"]\n");
        let m = merge_perms(base, over).unwrap();
        assert_eq!(
            m["allow"]["shell"].as_array().unwrap(),
            &vec![Value::from("cat"), Value::from("git status")]
        );
        assert_eq!(m["allow"]["read"][0].as_str(), Some("/srv/x"));
        assert_eq!(m["deny"]["shell_exact"][0].as_str(), Some("env"));
        let parsed: crate::model::PermFile = toml::from_str(&toml::to_string(&m).unwrap()).unwrap();
        assert_eq!(parsed.allow.shell, ["cat", "git status"]);
    }

    #[test]
    fn models_merge_by_key_and_validate() {
        let base = t("[vocabulary]\ncode = \"c\"\nold = \"o\"\n[[model]]\nmatch = \"*a*\"\ntier = \"fast\"\ntags = [\"code\"]\n[[model]]\nmatch = \"*b*\"\ntier = \"fast\"\ntags = [\"old\"]\n");
        let over = t("[remove]\nvocabulary = [\"old\"]\nmodels = [\"*b*\"]\n[vocabulary]\nnew = \"n\"\n[[model]]\nmatch = \"*a*\"\ntier = \"deep\"\ntags = [\"new\"]\n");
        let m = merge_models(base, over).unwrap();
        let f: crate::models::ModelsFile = toml::from_str(&toml::to_string(&m).unwrap()).unwrap();
        f.validate().unwrap();
        assert_eq!(f.models.len(), 1);
        assert_eq!(f.models[0].tier, "deep");
        assert!(f.vocabulary.contains_key("new") && !f.vocabulary.contains_key("old"));
    }

    #[test]
    fn map_deep_merges_and_unions_placeholders() {
        let base = t("placeholders = [\"A\"]\n[tiers.claude]\nfast = \"haiku\"\ndeep = \"opus\"\n");
        let over = t("placeholders = [\"B\", \"A\"]\n[tiers.claude]\nfast = \"other\"\n");
        let m = merge_map(base, over).unwrap();
        assert_eq!(
            m["placeholders"].as_array().unwrap(),
            &vec![Value::from("A"), Value::from("B")]
        );
        assert_eq!(m["tiers"]["claude"]["fast"].as_str(), Some("other"));
        assert_eq!(m["tiers"]["claude"]["deep"].as_str(), Some("opus"));
        assert!(merge_map(Table::new(), t("[remove]\nx = []\n")).is_err());
    }

    #[test]
    fn remove_entries_must_stay_inside_the_checkout() {
        for bad in ["", "/etc", "../x", "skills/../../x"] {
            assert!(safe_rel(bad).is_err(), "{bad}");
        }
        assert_eq!(safe_rel("skills/x/").unwrap(), PathBuf::from("skills/x"));
    }

    #[test]
    fn insert_drops_shadowed_paths() {
        let mut plan = Plan::default();
        plan.insert("skills/x/SKILL.md".into(), Item::Link("/b/1".into()));
        plan.insert("skills/x".into(), Item::Link("/o/x".into()));
        assert_eq!(plan.items.len(), 1);
        plan.insert("rules".into(), Item::Link("/f".into()));
        plan.insert("rules/core.md".into(), Item::Link("/o/c".into()));
        assert!(!plan.items.contains_key(Path::new("rules")));
        // A file over a directory drops the directory, so building never hits EEXIST.
        plan.dirs.insert("sync/commands".into());
        plan.dirs.insert("sync/commands/sub".into());
        plan.insert("sync/commands".into(), Item::Link("/o/cmds".into()));
        assert!(plan.dirs.is_empty());
    }

    #[test]
    fn a_link_inside_a_layer_is_a_leaf_never_walked() {
        let d = std::env::temp_dir().join(unique("agent-sync-ut-cycle"));
        let base = d.join("base");
        let over = d.join("over");
        std::fs::create_dir_all(base.join("rules")).unwrap();
        std::fs::create_dir_all(&over).unwrap();
        std::fs::write(base.join("rules/core.md"), "core").unwrap();
        // A cycle back to the checkout root: walking into it would never end.
        std::os::unix::fs::symlink("..", base.join("rules/loop")).unwrap();
        let layers = Layers {
            base,
            overlay: over,
        };
        let plan = plan(&layers, &|p| p.display().to_string()).unwrap();
        assert!(matches!(
            plan.items.get(Path::new("rules/loop")),
            Some(Item::Link(_))
        ));
        assert!(!plan
            .items
            .keys()
            .any(|k| k.starts_with("rules/loop") && k != Path::new("rules/loop")));
        let _ = std::fs::remove_dir_all(&d);
    }
}
