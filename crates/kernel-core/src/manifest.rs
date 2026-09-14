//! Application manifest parsing + validation (V0.7).
//!
//! An app declares its identity, version, and **requested capabilities** in a
//! small line-based manifest. The platform launches apps only through this
//! manifest: the process receives `manifest caps ∩ launcher caps` — default
//! deny, least privilege. The parser is strict: unknown keys, unknown
//! capabilities, duplicates, or malformed fields are ERRORS, never silently
//! tolerated (a typo must not become an unreviewed grant). Pure + host-tested
//! with adversarial inputs; allocation-free (fields borrow from the input).

use crate::caps;

pub const MAX_MANIFEST_LEN: usize = 1024;
pub const MAX_NAME_LEN: usize = 16;
pub const MAX_VERSION_LEN: usize = 16;

/// A parsed, validated application manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Manifest<'a> {
    pub name: &'a str,
    pub version: &'a str,
    /// Requested capability set (granted only if the launcher holds them).
    pub caps: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestError {
    TooLarge,
    /// A line is not `key=value` or a comment.
    Syntax,
    UnknownKey,
    DuplicateKey,
    MissingField,
    BadName,
    BadVersion,
    /// The caps list names an undefined capability.
    UnknownCapability,
}

/// A valid application name: 1-16 of `a-z`, `0-9` and `-`, not starting
/// with `-`. Also what the package store and `pkg launch` accept (V1.0).
pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_NAME_LEN
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
}

fn valid_version(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_VERSION_LEN
        && s.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && !s.starts_with('.')
        && !s.ends_with('.')
}

/// Parse and validate a manifest.
pub fn parse(text: &str) -> Result<Manifest<'_>, ManifestError> {
    if text.len() > MAX_MANIFEST_LEN {
        return Err(ManifestError::TooLarge);
    }
    let mut name = None;
    let mut version = None;
    let mut caps_val = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once('=').ok_or(ManifestError::Syntax)?;
        let (key, value) = (key.trim(), value.trim());
        match key {
            "name" => {
                if name.is_some() {
                    return Err(ManifestError::DuplicateKey);
                }
                if !valid_name(value) {
                    return Err(ManifestError::BadName);
                }
                name = Some(value);
            }
            "version" => {
                if version.is_some() {
                    return Err(ManifestError::DuplicateKey);
                }
                if !valid_version(value) {
                    return Err(ManifestError::BadVersion);
                }
                version = Some(value);
            }
            "caps" => {
                if caps_val.is_some() {
                    return Err(ManifestError::DuplicateKey);
                }
                caps_val = Some(caps::parse(value).map_err(|_| ManifestError::UnknownCapability)?);
            }
            _ => return Err(ManifestError::UnknownKey),
        }
    }
    Ok(Manifest {
        name: name.ok_or(ManifestError::MissingField)?,
        version: version.ok_or(ManifestError::MissingField)?,
        caps: caps_val.ok_or(ManifestError::MissingField)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::{CAP_FS_READ, CAP_GUI};

    const GOOD: &str = "# hello app\nname=hello-app\nversion=1.0.0\ncaps=gui,fs_read\n";

    #[test]
    fn parses_valid_manifest() {
        let m = parse(GOOD).unwrap();
        assert_eq!(m.name, "hello-app");
        assert_eq!(m.version, "1.0.0");
        assert_eq!(m.caps, CAP_GUI | CAP_FS_READ);
    }

    #[test]
    fn empty_caps_means_default_deny() {
        let m = parse("name=quiet\nversion=1\ncaps=\n").unwrap();
        assert_eq!(m.caps, 0);
    }

    #[test]
    fn rejects_missing_fields() {
        assert_eq!(
            parse("name=x\nversion=1\n"),
            Err(ManifestError::MissingField)
        );
        assert_eq!(parse(""), Err(ManifestError::MissingField));
    }

    #[test]
    fn rejects_unknown_key_and_capability() {
        assert_eq!(
            parse("name=x\nversion=1\ncaps=\nexec=/bin/sh\n"),
            Err(ManifestError::UnknownKey)
        );
        assert_eq!(
            parse("name=x\nversion=1\ncaps=root\n"),
            Err(ManifestError::UnknownCapability)
        );
    }

    #[test]
    fn rejects_duplicates_and_syntax() {
        assert_eq!(
            parse("name=a\nname=b\nversion=1\ncaps=\n"),
            Err(ManifestError::DuplicateKey)
        );
        assert_eq!(
            parse("name\nversion=1\ncaps=\n"),
            Err(ManifestError::Syntax)
        );
    }

    #[test]
    fn rejects_hostile_names_and_versions() {
        for bad in [
            "",
            "UPPER",
            "sp ace",
            "dot.dot",
            "a/../b",
            "-lead",
            "x".repeat(17).as_str(),
        ] {
            let text = format!("name={bad}\nversion=1\ncaps=\n");
            assert_eq!(parse(&text), Err(ManifestError::BadName), "name={bad:?}");
        }
        for bad in ["", "v1", "1..", ".1", "1.0-rc"] {
            let text = format!("name=x\nversion={bad}\ncaps=\n");
            assert_eq!(parse(&text), Err(ManifestError::BadVersion), "ver={bad:?}");
        }
    }

    #[test]
    fn rejects_oversized_manifest() {
        let big = format!(
            "name=x\nversion=1\ncaps=\n#{}\n",
            "y".repeat(MAX_MANIFEST_LEN)
        );
        assert_eq!(parse(&big), Err(ManifestError::TooLarge));
    }
}
