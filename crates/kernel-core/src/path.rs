//! Absolute path normalization for the VFS (requirement FS-001).
//!
//! Allocation-free: normalization walks components and yields them through a
//! callback-free iterator. Traversal that would escape the root (`..` at
//! root) is rejected rather than clamped, so hostile paths are surfaced
//! (plan §10.9).

/// Path validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathError {
    /// Path does not start with `/`.
    NotAbsolute,
    /// `..` traversal above the root.
    EscapesRoot,
    /// Component longer than the supported maximum (defensive bound).
    ComponentTooLong,
}

pub const MAX_COMPONENT: usize = 100;

/// Iterator over normalized components of an absolute path.
///
/// Normalization removes empty components (`//`), `.`, and resolves `..`
/// against previously yielded components — which requires lookahead, so this
/// iterator works on a two-pass scheme: [`validate`] first (which also
/// checks traversal), then iterate raw components skipping `.`/empty and
/// counting `..` pops via [`normalized`].
pub fn validate(path: &str) -> Result<(), PathError> {
    if !path.starts_with('/') {
        return Err(PathError::NotAbsolute);
    }
    let mut depth: i32 = 0;
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return Err(PathError::EscapesRoot);
                }
            }
            c => {
                if c.len() > MAX_COMPONENT {
                    return Err(PathError::ComponentTooLong);
                }
                depth += 1;
            }
        }
    }
    Ok(())
}

/// Yield normalized components of a validated path in order.
///
/// Caller must have run [`validate`] first; this function assumes traversal
/// is in-bounds and silently clamps otherwise (defense in depth).
pub fn normalized(path: &str) -> impl Iterator<Item = &str> {
    // Stack of component indices implemented over the input using a small
    // fixed array of slices (max depth bound keeps this allocation-free).
    const MAX_DEPTH: usize = 32;
    let mut stack: [&str; MAX_DEPTH] = [""; MAX_DEPTH];
    let mut len = 0usize;
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => len = len.saturating_sub(1),
            c => {
                if len < MAX_DEPTH {
                    stack[len] = c;
                    len += 1;
                }
            }
        }
    }
    let mut i = 0;
    core::iter::from_fn(move || {
        if i < len {
            let c = stack[i];
            i += 1;
            Some(c)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(path: &str) -> Vec<&str> {
        normalized(path).collect()
    }

    #[test]
    fn accepts_and_normalizes_simple_paths() {
        validate("/etc/version").unwrap();
        assert_eq!(norm("/etc/version"), vec!["etc", "version"]);
        assert_eq!(norm("/"), Vec::<&str>::new());
    }

    #[test]
    fn strips_empty_and_dot_components() {
        validate("//etc/./version/").unwrap();
        assert_eq!(norm("//etc/./version/"), vec!["etc", "version"]);
    }

    #[test]
    fn resolves_dotdot_within_bounds() {
        validate("/a/b/../c").unwrap();
        assert_eq!(norm("/a/b/../c"), vec!["a", "c"]);
    }

    #[test]
    fn rejects_relative_paths() {
        assert_eq!(validate("etc/version"), Err(PathError::NotAbsolute));
        assert_eq!(validate(""), Err(PathError::NotAbsolute));
    }

    #[test]
    fn rejects_root_escape() {
        assert_eq!(validate("/../etc"), Err(PathError::EscapesRoot));
        assert_eq!(validate("/a/../../b"), Err(PathError::EscapesRoot));
    }

    #[test]
    fn rejects_oversized_component() {
        let long = format!("/{}", "x".repeat(MAX_COMPONENT + 1));
        assert_eq!(validate(&long), Err(PathError::ComponentTooLong));
    }
}
