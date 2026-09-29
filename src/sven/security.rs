use std::env::current_dir;
use std::path::{Component, Path, PathBuf};

use crate::sven::security_error::SecurityError;

/// Check that `path` (relative or absolute) stays inside the current
/// working directory. Symlinks are resolved, so a link inside the
/// workspace that points outside cannot be used to escape.
pub fn is_inside_cwd(path: &str) -> Result<(), SecurityError> {
    if path.is_empty() {
        return Err(SecurityError::EmptyPath);
    }

    let cwd = current_dir()?;
    let resolved = resolve(Path::new(path))?;
    if !resolved.starts_with(&cwd) {
        return Err(SecurityError::Unauthorized(path.to_string()));
    }
    Ok(())
}

/// Resolve `path` to an absolute, symlink-free path. Existing components
/// are canonicalized one by one — so symlinks are followed and `..` is
/// applied to the *physical* path — while components that do not exist
/// yet (a file about to be created) are appended as they are written.
///
/// A component that does not exist but is a *dangling symlink* is
/// resolved through its target: writing through it must not be able to
/// land outside the workspace. Symlink loops fail closed via
/// `canonicalize`'s `ELOOP` before any recursion can cycle.
fn resolve(path: &Path) -> Result<PathBuf, SecurityError> {
    let mut resolved = if path.is_absolute() {
        PathBuf::from("/")
    } else {
        current_dir()?
    };

    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop(); // stops at the root
            }
            Component::Normal(name) => {
                resolved.push(name);
                match std::fs::canonicalize(&resolved) {
                    Ok(canonical) => resolved = canonical,
                    // not created yet — keep the lexical component, unless
                    // it is a dangling symlink (see above)
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        if let Ok(target) = std::fs::read_link(&resolved) {
                            resolved.pop();
                            let base = if target.is_absolute() {
                                PathBuf::from("/")
                            } else {
                                resolved
                            };
                            resolved = resolve(&base.join(target))?;
                        }
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            // a Windows drive prefix can never be inside the cwd
            Component::Prefix(_) => {
                return Err(SecurityError::Unauthorized(path.display().to_string()))
            }
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    /// Confinement regression tests: traversal, absolute paths and symlink
    /// escapes must all be rejected. The cwd is process-global, so
    /// everything runs sequentially in one test inside a throwaway
    /// directory.
    #[test]
    fn confines_paths_to_the_working_directory() {
        let base = std::env::temp_dir();
        let dir = base.join(format!("sven-security-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("inside.txt"), "x").unwrap();
        fs::create_dir_all(dir.join("sub")).unwrap();
        symlink(&base, dir.join("link")).unwrap();
        symlink(dir.join("inside.txt"), dir.join("in-link")).unwrap();
        // dangling symlink pointing outside the workspace
        let outside = base.join(format!("sven-security-outside-{}", std::process::id()));
        let _ = fs::remove_file(&outside);
        symlink(&outside, dir.join("dangling")).unwrap();

        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        // paths inside the working directory are allowed, including
        // not-yet-existing tails (files about to be created)
        assert!(is_inside_cwd("inside.txt").is_ok());
        assert!(is_inside_cwd("./sub/../inside.txt").is_ok());
        assert!(is_inside_cwd("sub/new.txt").is_ok());
        assert!(is_inside_cwd("in-link").is_ok());

        // traversal, absolute paths and empty paths are rejected
        assert!(is_inside_cwd("../../etc/hostname").is_err());
        assert!(is_inside_cwd("/etc/hostname").is_err());
        assert!(is_inside_cwd("").is_err());

        // symlinks resolving outside the working directory are rejected —
        // existing ones …
        assert!(is_inside_cwd("link").is_err());
        assert!(is_inside_cwd("link/secret.txt").is_err());
        // … and dangling ones a write would follow
        assert!(is_inside_cwd("dangling").is_err());
        assert!(is_inside_cwd("dangling/new.txt").is_err());

        std::env::set_current_dir(previous).unwrap();
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_file(&outside);
    }
}