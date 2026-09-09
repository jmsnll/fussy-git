//! Filesystem mutations shared by the commands that relocate or delete
//! repositories (`reconcile`, `adopt`, `remove`).
//!
//! Nothing here touches repository *contents* — a move is a directory rename
//! (with a copy+verify+delete fallback across filesystems), and a delete is a
//! plain recursive remove. Both tidy up the now-empty parent directories they
//! leave behind, without ever climbing past a managed root.

use std::path::Path;

use crate::config::Config;

/// Rename `from` to `to`, falling back to a recursive copy + verify + delete
/// when the two live on different filesystems (`EXDEV`).
pub fn move_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        // EXDEV is 18 on both Linux and macOS.
        Err(e) if e.raw_os_error() == Some(18) => {
            copy_dir_all(from, to)?;
            verify_copy(from, to)?;
            std::fs::remove_dir_all(from)?;
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Recursively delete `repo`, then prune any parent directories that became
/// empty, stopping before the managed root that contains it.
pub fn remove_tree(cfg: &Config, repo: &Path) -> std::io::Result<()> {
    std::fs::remove_dir_all(repo)?;
    if let Some(root) = managed_root(cfg, repo) {
        prune_empty_dirs(repo.parent(), root);
    }
    Ok(())
}

pub fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&from, &to)?;
        } else if file_type.is_symlink() {
            let target = std::fs::read_link(&from)?;
            std::os::unix::fs::symlink(target, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

pub fn count_entries(dir: &Path) -> std::io::Result<usize> {
    let mut total = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        total += 1;
        if entry.file_type()?.is_dir() {
            total += count_entries(&entry.path())?;
        }
    }
    Ok(total)
}

/// Shallow sanity check after a cross-device copy: the `.git` entry made it and
/// the recursive entry count matches.
pub fn verify_copy(from: &Path, to: &Path) -> std::io::Result<()> {
    if !to.join(".git").exists() {
        return Err(std::io::Error::other(
            "copy verification failed: .git missing at destination",
        ));
    }
    let (src, dst) = (count_entries(from)?, count_entries(to)?);
    if src != dst {
        return Err(std::io::Error::other(format!(
            "copy verification failed: {src} entries at source, {dst} at destination"
        )));
    }
    Ok(())
}

/// The managed root (longest prefix of `path` among `cfg.roots`).
pub fn managed_root<'a>(cfg: &'a Config, path: &Path) -> Option<&'a Path> {
    cfg.roots
        .iter()
        .map(|p| p.as_path())
        .filter(|r| path.starts_with(r))
        .max_by_key(|r| r.components().count())
}

/// Remove now-empty ancestor directories of `start`, stopping before
/// `stop_root` (and never removing it).
pub fn prune_empty_dirs(start: Option<&Path>, stop_root: &Path) {
    let mut dir = start;
    while let Some(d) = dir {
        if d == stop_root || !d.starts_with(stop_root) {
            break;
        }
        match std::fs::read_dir(d) {
            Ok(mut rd) => {
                if rd.next().is_some() {
                    break;
                }
            }
            Err(_) => break,
        }
        if std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_rooted(root: &Path) -> Config {
        let mut c = Config::default();
        c.root = root.to_path_buf();
        c.roots = vec![root.to_path_buf()];
        c
    }

    #[test]
    fn move_dir_renames_within_fs() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("a/b");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("f"), "x").unwrap();
        let to = tmp.path().join("c/d");
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        move_dir(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read_to_string(to.join("f")).unwrap(), "x");
    }

    #[test]
    fn prune_stops_at_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let deep = root.join("x/y/z");
        std::fs::create_dir_all(&deep).unwrap();
        prune_empty_dirs(Some(&deep), root);
        assert!(root.exists());
        assert!(!root.join("x").exists());
    }

    #[test]
    fn prune_keeps_non_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let deep = root.join("x/y/z");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(root.join("x/keep"), "1").unwrap();
        prune_empty_dirs(Some(&deep), root);
        assert!(root.join("x").exists());
        assert!(!root.join("x/y").exists());
    }

    #[test]
    fn remove_tree_deletes_and_prunes() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_rooted(tmp.path());
        let repo = tmp.path().join("host/owner/repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        remove_tree(&cfg, &repo).unwrap();
        assert!(!repo.exists());
        assert!(!tmp.path().join("host").exists());
        assert!(tmp.path().exists());
    }

    #[test]
    fn managed_root_picks_longest_prefix() {
        let mut cfg = Config::default();
        cfg.roots = vec![
            Path::new("/a").to_path_buf(),
            Path::new("/a/b").to_path_buf(),
        ];
        assert_eq!(
            managed_root(&cfg, Path::new("/a/b/c/repo")),
            Some(Path::new("/a/b"))
        );
    }
}
