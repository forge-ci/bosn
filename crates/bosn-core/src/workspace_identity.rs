//! Pure workspace identity spelling over caller-observed filesystem facts (#777).

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathStyle {
    Posix,
    Windows,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathCase {
    Sensitive,
    Insensitive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityError {
    NotAbsolute,
    UnsupportedVerbatim,
}

impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotAbsolute => "workspace identity requires an absolute path",
            Self::UnsupportedVerbatim => "workspace identity does not support this verbatim path",
        })
    }
}
impl std::error::Error for IdentityError {}

/// Case-sensitive POSIX input is byte-identical to the legacy identity. Callers resolve
/// links first; this function performs no filesystem or host-platform guessing.
pub fn canonical_root(
    raw: &str,
    style: PathStyle,
    case: PathCase,
) -> Result<String, IdentityError> {
    let root = match style {
        PathStyle::Posix if raw.starts_with('/') => raw.to_owned(),
        PathStyle::Posix => return Err(IdentityError::NotAbsolute),
        PathStyle::Windows => windows_root(raw)?,
    };
    Ok(match case {
        PathCase::Sensitive => root,
        PathCase::Insensitive => root.to_ascii_lowercase(),
    })
}

fn windows_root(raw: &str) -> Result<String, IdentityError> {
    // A single backslash is current-drive rooted, never an MSYS drive spelling.
    if raw.starts_with('\\') && !raw.starts_with("\\\\") {
        return Err(IdentityError::NotAbsolute);
    }
    let slashed = raw.replace('\\', "/");
    let ordinary = if slashed
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("//?/UNC/"))
    {
        format!("//{}", &slashed[8..])
    } else if let Some(rest) = slashed.strip_prefix("//?/") {
        if !drive_path(rest) {
            return Err(IdentityError::UnsupportedVerbatim);
        }
        rest.to_owned()
    } else {
        slashed
    };
    let drive = if let Some(rest) = ordinary.strip_prefix("/mnt/") {
        unix_drive(rest)
    } else {
        unix_drive(ordinary.strip_prefix('/').unwrap_or(""))
    };
    let ordinary = drive.unwrap_or(ordinary);
    let (anchor, rest) = if drive_path(&ordinary) {
        (
            format!("{}:/", ordinary[..1].to_ascii_uppercase()),
            &ordinary[3..],
        )
    } else if let Some(unc) = ordinary.strip_prefix("//") {
        let mut pieces = unc.splitn(3, '/');
        let server = pieces
            .next()
            .filter(|p| valid_anchor(p))
            .ok_or(IdentityError::NotAbsolute)?;
        let share = pieces
            .next()
            .filter(|p| valid_anchor(p))
            .ok_or(IdentityError::NotAbsolute)?;
        (format!("//{server}/{share}/"), pieces.next().unwrap_or(""))
    } else {
        return Err(IdentityError::NotAbsolute);
    };
    let mut parts = Vec::new();
    for part in rest.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    let result = format!("{anchor}{}", parts.join("/"));
    Ok(if parts.is_empty() && anchor.starts_with("//") {
        result.trim_end_matches('/').to_owned()
    } else {
        result
    })
}

fn valid_anchor(part: &str) -> bool {
    !matches!(part, "" | "." | "..")
}

fn drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1..3] == *b":/"
}

fn unix_drive(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    (bytes.first().is_some_and(u8::is_ascii_alphabetic)
        && (bytes.len() == 1 || bytes.get(1) == Some(&b'/')))
    .then(|| {
        format!(
            "{}:/{}",
            path[..1].to_ascii_uppercase(),
            path.get(2..).unwrap_or("")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_drive_spellings_converge() {
        for raw in [
            r"C:\Dev\Foo",
            "C:/Dev/Foo",
            "/c/Dev/Foo",
            "/mnt/c/Dev/Foo",
            r"\\?\C:\Dev\Foo",
            "c:/Dev/./other/../Foo/",
        ] {
            assert_eq!(
                canonical_root(raw, PathStyle::Windows, PathCase::Insensitive).unwrap(),
                "c:/dev/foo",
                "{raw}"
            );
        }
    }

    #[test]
    fn unc_long_and_ordinary_forms_converge() {
        for raw in [
            r"\\Server\Share\Foo",
            r"\\?\UNC\Server\Share\Foo",
            "//server/share/foo/",
        ] {
            assert_eq!(
                canonical_root(raw, PathStyle::Windows, PathCase::Insensitive).unwrap(),
                "//server/share/foo"
            );
        }
    }

    #[test]
    fn case_comes_from_filesystem_facts_and_linux_bytes_do_not_change() {
        for raw in ["/home/User/Repo", "/tmp/repo/", r"/home/user/back\slash"] {
            assert_eq!(
                canonical_root(raw, PathStyle::Posix, PathCase::Sensitive).unwrap(),
                raw
            );
        }
        assert_eq!(
            canonical_root("/Users/Me/Repo", PathStyle::Posix, PathCase::Insensitive).unwrap(),
            "/users/me/repo"
        );
        assert_eq!(
            canonical_root("C:/Dev/Foo", PathStyle::Windows, PathCase::Sensitive).unwrap(),
            "C:/Dev/Foo"
        );
        assert_eq!(
            canonical_root("/mnt/c/Dev/Foo", PathStyle::Posix, PathCase::Sensitive).unwrap(),
            "/mnt/c/Dev/Foo"
        );
    }

    #[test]
    fn invalid_paths_fail_at_the_boundary() {
        for raw in [
            "relative",
            "C:relative",
            r"\c",
            r"\c\repo",
            "C:",
            "//server",
            "//server/../foo",
            "/not-a-drive/foo",
        ] {
            assert!(
                canonical_root(raw, PathStyle::Windows, PathCase::Insensitive).is_err(),
                "{raw}"
            );
        }
        assert_eq!(
            canonical_root(
                r"\\?\Volume{abc}\foo",
                PathStyle::Windows,
                PathCase::Sensitive
            ),
            Err(IdentityError::UnsupportedVerbatim)
        );
        assert!(canonical_root("relative", PathStyle::Posix, PathCase::Sensitive).is_err());
    }
    #[test]
    fn ascii_case_evidence_never_merges_distinct_unicode_names() {
        let first = canonical_root("C:/Fooİ", PathStyle::Windows, PathCase::Insensitive).unwrap();
        let second =
            canonical_root("C:/Fooi\u{307}", PathStyle::Windows, PathCase::Insensitive).unwrap();
        assert_ne!(first, second);
        assert_eq!(first, "c:/fooİ");
    }
}
