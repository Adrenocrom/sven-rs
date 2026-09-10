use std::env::current_dir;
use std::path::Path;

use crate::sven::security_error::SecurityError;

pub fn is_inside_cwd(path: &str) -> Result<(), SecurityError> {
    if path.is_empty() {
        return Err(SecurityError::EmptyPath);
    }

    let path = Path::new(path);
    let cwd = current_dir()?;

    let normalized = path.normalize_lexically()?;
    let absolute = normalized.absolute()?;
    let is_inside = absolute.starts_with(&cwd);
    if !is_inside {
        println!(
            "Path {:?} is not inside current directory {:?}",
            &absolute, &cwd
        );
        return Err(SecurityError::Unauthorized);
    }
    Ok(())
}
