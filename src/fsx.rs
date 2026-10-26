//! Every write agent-sync makes goes through here.
//!
//! - **Known roots only.** A destination must resolve (after following any symlinked parent
//!   directory) inside one of the roots registered with [`allow_writes_under`]: the tool
//!   directories, the source checkout and the state directory. A symlinked parent that leads
//!   anywhere else is refused, as is a destination that is itself a symlink.
//! - **Exclusive temporaries.** The new content and the backup are each written to a fresh file
//!   created with `O_CREAT | O_EXCL` (which never follows a symlink) under a random name in the
//!   destination's directory, then renamed into place. A symlink planted at the backup or a
//!   temporary path is replaced, never written through.
//! - **One writer at a time.** [`Lock`] is an exclusive `flock` that `apply` (and every other
//!   writing command) holds for its whole run.
//!
//! Residual risk, accepted: the checks work on paths, not directory handles. A process running
//! as the same user that swaps a parent directory for a symlink between the check and the
//! rename can redirect a write; such a process can already edit these files directly. Only the
//! final open (`O_EXCL`) is guaranteed not to follow a symlink.

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::RwLock;

static WRITE_ROOTS: RwLock<Vec<PathBuf>> = RwLock::new(Vec::new());
/// Single files outside any root that may be written (`~/.claude.json` lives in `$HOME`).
static WRITE_FILES: RwLock<Vec<PathBuf>> = RwLock::new(Vec::new());

/// Allow writes to exactly this file (additive). Returns false, allowing nothing, when the path
/// cannot be resolved.
pub fn allow_file(path: &Path) -> bool {
    let Some(r) = resolve_root(path) else {
        return false;
    };
    let mut w = WRITE_FILES
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !w.contains(&r) {
        w.push(r);
    }
    true
}

fn allowed(path: &Path) -> Result<()> {
    let parent = path.parent().context("path has no parent")?;
    let Some(dir) = resolve(parent) else {
        bail!(
            "refusing to write {}: {} goes through a symlink that cannot be resolved",
            path.display(),
            parent.display()
        );
    };
    let file = dir.join(path.file_name().context("path has no file name")?);
    let in_root = WRITE_ROOTS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .any(|r| dir.starts_with(r));
    let listed = WRITE_FILES
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(&file);
    if !in_root && !listed {
        bail!(
            "refusing to write {}: its directory resolves to {}, outside the tool directories \
             agent-sync manages",
            path.display(),
            dir.display()
        );
    }
    Ok(())
}

/// Allow writes under `roots` (additive). Nothing can be written before this is called. A root
/// that cannot be resolved (relative, empty, or through a dangling or looping symlink) is
/// dropped, never widened; the dropped roots are returned so the caller can warn.
pub fn allow_writes_under<I: IntoIterator<Item = PathBuf>>(roots: I) -> Vec<PathBuf> {
    let mut dropped = Vec::new();
    let mut w = WRITE_ROOTS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for root in roots {
        match resolve_root(&root) {
            Some(r) if !w.contains(&r) => w.push(r),
            Some(_) => {}
            None => dropped.push(root),
        }
    }
    dropped
}

/// A root, resolved: absolute, non-empty, and reached without an unresolvable link.
fn resolve_root(root: &Path) -> Option<PathBuf> {
    if !root.is_absolute() || root.components().any(|c| c == Component::ParentDir) {
        return None;
    }
    resolve(root).filter(|r| r.is_absolute() && r.parent().is_some())
}

/// `path` with its longest existing prefix canonicalised (symlinks followed) and the rest
/// appended lexically. `None` when the path runs into a link that cannot be followed (dangling,
/// or a loop): such a path leads nowhere agent-sync can vouch for.
fn resolve(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
        return None;
    }
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    loop {
        if let Ok(c) = std::fs::canonicalize(&existing) {
            let mut out = c;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return Some(out);
        }
        if std::fs::symlink_metadata(&existing).is_ok() {
            return None;
        }
        match (
            existing.file_name().map(|n| n.to_os_string()),
            existing.parent(),
        ) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => return None,
        }
    }
}

/// Refuse a destination outside the registered roots, or one that is itself a symlink.
pub fn check_destination(path: &Path) -> Result<()> {
    if path.components().any(|c| c == Component::ParentDir) || !path.is_absolute() {
        bail!(
            "refusing to write {}: not a plain absolute path",
            path.display()
        );
    }
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        bail!(
            "refusing to write {}: it is a symlink (agent-sync replaces files, it does not \
             write through links)",
            path.display()
        );
    }
    allowed(path)
}

/// A name unlikely to collide: pid, time and a process-wide counter. It is not secret; safety
/// comes from creating the file with `O_EXCL`, which fails on any existing entry, link included.
pub fn unique(stem: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!(
        "{stem}.{}-{nanos}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

/// A sibling of `path` with a fresh random suffix, for a temporary file or link.
fn sibling(path: &Path, tag: &str) -> Result<PathBuf> {
    let name = path
        .file_name()
        .context("path has no file name")?
        .to_string_lossy();
    Ok(path.with_file_name(unique(&format!(".{name}.agent-sync-{tag}"))))
}

/// Create a new file next to `path` with `bytes` and `mode`, exclusively. Returns its path.
fn write_exclusive(path: &Path, tag: &str, bytes: &[u8], mode: u32) -> Result<PathBuf> {
    for _ in 0..8 {
        let tmp = sibling(path, tag)?;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)
        {
            Ok(mut f) => {
                let done = f
                    .write_all(bytes)
                    .and_then(|()| f.sync_all())
                    .and_then(|()| {
                        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))
                    });
                if let Err(e) = done {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(e).with_context(|| format!("writing {}", tmp.display()));
                }
                return Ok(tmp);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("creating {}", tmp.display())),
        }
    }
    bail!(
        "could not create a temporary file next to {}",
        path.display()
    )
}

/// Rename `tmp` over `dest`, removing `tmp` if that fails.
fn rename_into(tmp: &Path, dest: &Path) -> Result<()> {
    std::fs::rename(tmp, dest).map_err(|e| {
        let _ = std::fs::remove_file(tmp);
        anyhow::Error::from(e).context(format!("renaming into {}", dest.display()))
    })
}

/// The backup kept next to a replaced file.
pub fn backup_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.agent-sync.bak", path.display()))
}

/// Replace `path` with `content`: a fresh temporary file renamed into place. An existing file is
/// first copied the same way to `<path>.agent-sync.bak`, and its mode is kept.
pub fn atomic_write(path: &Path, content: &str) -> Result<()> {
    check_destination(path)?;
    let dir = path.parent().context("path has no parent")?;
    std::fs::create_dir_all(dir)?;
    // The directory may have been created through a symlink just now: check again.
    check_destination(path)?;
    let old = match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() => Some((m.permissions().mode() & 0o7777, std::fs::read(path)?)),
        Ok(_) => bail!("refusing to write {}: not a regular file", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mode = old.as_ref().map_or(0o644, |(m, _)| *m);
    if let Some((m, bytes)) = &old {
        let bak = backup_path(path);
        if std::fs::symlink_metadata(&bak).is_ok_and(|b| b.is_dir()) {
            bail!("{} is a directory; move it aside by hand", bak.display());
        }
        let tmp = write_exclusive(path, "bak", bytes, *m)?;
        rename_into(&tmp, &bak).with_context(|| format!("backing up {}", path.display()))?;
    }
    let tmp = write_exclusive(path, "tmp", content.as_bytes(), mode)?;
    rename_into(&tmp, path)
}

/// Point `link` at `target` with a fresh symlink renamed over it, so the link is never absent.
/// A real file or directory at `link` is refused.
pub fn replace_link(link: &Path, target: &Path) -> Result<()> {
    check_destination_dir(link)?;
    match std::fs::symlink_metadata(link) {
        Ok(m) if !m.file_type().is_symlink() => bail!(
            "{} is a real file/dir; move it aside by hand first",
            link.display()
        ),
        _ => {}
    }
    let dir = link.parent().context("link has no parent")?;
    std::fs::create_dir_all(dir)?;
    check_destination_dir(link)?;
    for _ in 0..8 {
        let tmp = sibling(link, "link")?;
        match std::os::unix::fs::symlink(target, &tmp) {
            Ok(()) => return rename_into(&tmp, link),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("linking {}", tmp.display())),
        }
    }
    bail!(
        "could not create a temporary link next to {}",
        link.display()
    )
}

/// Remove a link agent-sync owns. Anything that is not a symlink is left alone.
pub fn remove_link(link: &Path) -> Result<bool> {
    check_destination_dir(link)?;
    match std::fs::symlink_metadata(link) {
        Ok(m) if m.file_type().is_symlink() => {
            std::fs::remove_file(link)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Remove a generated file agent-sync owns.
pub fn remove_file(path: &Path) -> Result<()> {
    check_destination(path)?;
    std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))
}

/// Like [`check_destination`], but the destination itself may be a symlink (it is a link).
fn check_destination_dir(link: &Path) -> Result<()> {
    if link.components().any(|c| c == Component::ParentDir) || !link.is_absolute() {
        bail!(
            "refusing to write {}: not a plain absolute path",
            link.display()
        );
    }
    allowed(link)
}

/// An exclusive lock on `<state>/lock`, held until dropped. A second writer waits for it.
pub struct Lock(#[allow(dead_code)] std::fs::File);

impl Lock {
    pub fn acquire(state: &Path) -> Result<Lock> {
        refuse_symlink(state)?;
        std::fs::create_dir_all(state).with_context(|| format!("creating {}", state.display()))?;
        let path = state.join("lock");
        refuse_symlink(&path)?;
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        // The open may have raced a link into place: the file locked must be the path itself.
        {
            use std::os::unix::fs::MetadataExt;
            let opened = f.metadata()?;
            let at = std::fs::symlink_metadata(&path)?;
            if at.file_type().is_symlink() || at.ino() != opened.ino() || at.dev() != opened.dev() {
                bail!(
                    "{} changed while it was opened; refusing it",
                    path.display()
                );
            }
        }
        f.lock()
            .with_context(|| format!("locking {}", path.display()))?;
        Ok(Lock(f))
    }
}

/// Refuse a state path that is a symlink: the state directory is ours, links in it are not.
pub fn refuse_symlink(path: &Path) -> Result<()> {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        bail!(
            "{} is a symlink; agent-sync refuses to follow it",
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(unique(&format!("agent-sync-fsx-{name}")));
        std::fs::create_dir_all(&d).unwrap();
        allow_writes_under([d.clone()]);
        d
    }

    #[test]
    fn planted_symlinks_at_backup_and_temp_paths_are_never_written_through() {
        let d = tmp("planted");
        let victim = d.join("victim");
        std::fs::write(&victim, "untouched").unwrap();
        let dest = d.join("settings.json");
        std::fs::write(&dest, "old").unwrap();
        std::os::unix::fs::symlink(&victim, backup_path(&dest)).unwrap();
        // The old fixed temp name, planted too.
        std::os::unix::fs::symlink(&victim, d.join("settings.json.agent-sync.tmp")).unwrap();
        atomic_write(&dest, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
        // The backup replaced the planted link with a real file.
        let bak = backup_path(&dest);
        assert!(!std::fs::symlink_metadata(&bak)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "old");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_symlinked_destination_or_directory_outside_the_roots_is_refused() {
        let d = tmp("outside");
        let outside = std::env::temp_dir().join(unique("agent-sync-fsx-elsewhere"));
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, d.join("escape")).unwrap();
        assert!(atomic_write(&d.join("escape/x.json"), "x").is_err());
        assert!(!outside.join("x.json").exists());
        std::fs::write(outside.join("t"), "t").unwrap();
        std::os::unix::fs::symlink(outside.join("t"), d.join("dest")).unwrap();
        assert!(atomic_write(&d.join("dest"), "x").is_err());
        assert_eq!(std::fs::read_to_string(outside.join("t")).unwrap(), "t");
        assert!(atomic_write(&d.join("../x"), "x").is_err());
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn a_dangling_root_is_dropped_and_never_widens_containment() {
        // Not registered itself: only the dangling root below it is.
        let d = std::env::temp_dir().join(unique("agent-sync-fsx-dangling"));
        std::fs::create_dir_all(&d).unwrap();
        let root = d.join("agent-terminal");
        std::os::unix::fs::symlink(d.join("does-not-exist"), &root).unwrap();
        assert_eq!(allow_writes_under([root.clone()]), vec![root]);
        assert!(!allow_file(Path::new("relative/file")));
        assert_eq!(
            allow_writes_under([PathBuf::new(), PathBuf::from("rel")]).len(),
            2
        );
        // An agents directory that escapes through a symlink stays refused.
        let outside = std::env::temp_dir().join(unique("agent-sync-fsx-outside"));
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, d.join("agents")).unwrap();
        assert!(atomic_write(&d.join("agents/x.md"), "x").is_err());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn replace_link_swaps_without_a_gap_and_refuses_real_files() {
        let d = tmp("link");
        let link = d.join("l");
        replace_link(&link, Path::new("/a")).unwrap();
        replace_link(&link, Path::new("/b")).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), PathBuf::from("/b"));
        std::fs::write(d.join("real"), "x").unwrap();
        assert!(replace_link(&d.join("real"), Path::new("/a")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_lock_serialises_two_holders() {
        let d = tmp("lock");
        let first = Lock::acquire(&d).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let d2 = d.clone();
        let h = std::thread::spawn(move || {
            let _second = Lock::acquire(&d2).unwrap();
            tx.send(()).unwrap();
        });
        // While the first holder lives, the second waits.
        assert!(rx
            .recv_timeout(std::time::Duration::from_millis(200))
            .is_err());
        drop(first);
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }
}
