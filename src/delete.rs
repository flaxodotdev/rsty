use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::util::home;

/// Refuse to touch anything that would be catastrophic.
pub fn guard(path: &Path, roots: &[PathBuf]) -> Result<(), String> {
    let h = home();
    if path.parent().is_none() || path.components().count() < 3 {
        return Err("refusing: path too close to filesystem root".into());
    }
    if path == h || h.starts_with(path) {
        return Err("refusing: that would delete your home directory".into());
    }
    if roots.iter().any(|r| r == path) {
        return Err("refusing: that's a scan root".into());
    }
    Ok(())
}

pub fn remove(path: &Path, clear_contents: bool, to_trash: bool, dry_run: bool, roots: &[PathBuf]) -> Result<bool, String> {
    guard(path, roots)?;
    if dry_run {
        std::thread::sleep(std::time::Duration::from_millis(300));
        return Ok(to_trash && !clear_contents);
    }
    if !path.exists() && fs::symlink_metadata(path).is_err() {
        return Ok(false); // already gone
    }
    if clear_contents {
        let kids: Vec<PathBuf> = fs::read_dir(path)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        kids.par_iter().try_for_each(|k| rm(k)).map_err(|e| e.to_string())?;
        return Ok(false);
    }
    if to_trash {
        trash_it(path).map_err(|e| e.to_string())?;
        return Ok(true);
    }
    rm(path).map_err(|e| format!("{e}"))?;
    Ok(false)
}

#[cfg(target_os = "macos")]
fn trash_it(p: &Path) -> Result<(), trash::Error> {
    use trash::macos::{DeleteMethod, TrashContextExtMacos};
    let mut ctx = trash::TrashContext::default();
    ctx.set_delete_method(DeleteMethod::NsFileManager);
    ctx.delete(p)
}

#[cfg(not(target_os = "macos"))]
fn trash_it(p: &Path) -> Result<(), trash::Error> {
    trash::delete(p)
}

/// Parallel recursive delete that also handles read-only directories
/// (Go's module cache, some cargo git checkouts).
fn rm(p: &Path) -> io::Result<()> {
    let md = fs::symlink_metadata(p)?;
    if !md.is_dir() {
        return match fs::remove_file(p) {
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                // unix: the parent dir is read-only; windows: the file has the read-only flag
                if let Some(parent) = p.parent() {
                    let _ = make_writable(parent);
                }
                let _ = make_writable(p);
                fs::remove_file(p)
            }
            r => r,
        };
    }
    if !writable(&md) {
        make_writable(p)?;
    }
    let kids: Vec<PathBuf> = fs::read_dir(p)?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    kids.par_iter().try_for_each(|k| rm(k))?;
    fs::remove_dir(p)
}

#[cfg(unix)]
fn writable(md: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    md.permissions().mode() & 0o200 != 0
}

#[cfg(not(unix))]
fn writable(md: &fs::Metadata) -> bool {
    !md.permissions().readonly()
}

#[cfg(unix)]
fn make_writable(p: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::symlink_metadata(p)?.permissions().mode();
    fs::set_permissions(p, fs::Permissions::from_mode(mode | 0o700))
}

#[cfg(not(unix))]
fn make_writable(p: &Path) -> io::Result<()> {
    let mut perm = fs::symlink_metadata(p)?.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    fs::set_permissions(p, perm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_blocks_home_and_root() {
        assert!(guard(Path::new("/"), &[]).is_err());
        assert!(guard(home(), &[]).is_err());
        assert!(guard(home().parent().unwrap(), &[]).is_err());
        assert!(guard(&home().join("x/target"), &[]).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn removes_readonly_tree() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = std::env::temp_dir().join(format!("rsty-rm-{}", std::process::id()));
        let ro = tmp.join("a/b");
        fs::create_dir_all(&ro).unwrap();
        fs::write(ro.join("f"), b"x").unwrap();
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).unwrap();
        fs::set_permissions(tmp.join("a"), fs::Permissions::from_mode(0o555)).unwrap();
        rm(&tmp).unwrap();
        assert!(!tmp.exists());
    }
}
