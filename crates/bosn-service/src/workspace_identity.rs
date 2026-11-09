//! Observe path facts at the service boundary; the core transform stays pure (#777).
use std::io;
use std::path::{Path, PathBuf};

use bosn_core::workspace_identity::{IdentityError, PathCase, PathStyle, canonical_root};
use kernal_api::platform::fs;

fn native_style() -> PathStyle {
    if cfg!(windows) {
        PathStyle::Windows
    } else {
        PathStyle::Posix
    }
}

/// Accept drive, MSYS, WSL and verbatim spellings before native canonicalization.
pub(crate) fn native_path(path: &Path) -> io::Result<PathBuf> {
    input_path(path, native_style())
}

fn input_path(path: &Path, style: PathStyle) -> io::Result<PathBuf> {
    if style == PathStyle::Posix {
        return Ok(path.to_path_buf());
    }
    match canonical_root(text(path)?, style, PathCase::Sensitive) {
        Ok(identity) => Ok(PathBuf::from(identity)),
        // Relative and current-drive rooted inputs are resolved by native canonicalization.
        Err(IdentityError::NotAbsolute) => Ok(path.to_path_buf()),
        Err(error) => Err(io::Error::other(error)),
    }
}

/// Observe identity off the async lane, then pass only facts to the pure volume planner.
pub(crate) async fn plan_volumes(
    stack: &bosn_core::manifest::Stack,
    workspace: &Path,
    generation: &str,
) -> Result<Vec<crate::ManifestVolumeResource>, String> {
    let identity_path = workspace.to_path_buf();
    let identity = kernal_api::async_engine::launch_blocking(move || observe(&identity_path))
        .await
        .map_err(|_| "workspace identity worker failed".to_owned())?
        .map_err(|error| format!("workspace identity unavailable: {error}"))?;
    crate::manifest_runtime::manifest_named_volumes(
        stack,
        text(workspace).map_err(|error| error.to_string())?,
        &identity,
        generation,
    )
}

/// The path must already be canonical. This runs on a blocking worker.
pub(crate) fn observe(path: &Path) -> io::Result<String> {
    canonical_root(text(path)?, native_style(), observe_case(path)?).map_err(io::Error::other)
}

fn text(path: &Path) -> io::Result<&str> {
    path.to_str()
        .ok_or_else(|| io::Error::other("workspace path is not UTF-8"))
}

/// Compare native object identities for alternate case spellings, never an OS default.
/// An alternate symlink is not evidence of case-insensitive lookup.
fn observe_case(path: &Path) -> io::Result<PathCase> {
    let mut observed = false;
    for component in path.ancestors() {
        let Some(name) = component.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some((offset, ch)) = name.char_indices().find(|(_, ch)| ch.is_ascii_alphabetic())
        else {
            continue;
        };
        let mut alternate = name.to_owned();
        alternate.replace_range(
            offset..offset + 1,
            &if ch.is_ascii_lowercase() {
                ch.to_ascii_uppercase().to_string()
            } else {
                ch.to_ascii_lowercase().to_string()
            },
        );
        let alternate = component.with_file_name(alternate);
        let original = fs::path_file::file_identity(component)?;
        match fs::context_path_metadata_no_follow(&alternate) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(PathCase::Sensitive);
            }
            Err(error) => return Err(error),
            Ok(metadata) if metadata.kind == fs::ContextPathKind::Symlink => {
                return Ok(PathCase::Sensitive);
            }
            Ok(_) => {
                if fs::path_file::file_identity(&alternate)? != original {
                    return Ok(PathCase::Sensitive);
                }
                observed = true;
            }
        }
    }
    // Fold only when every cased component has positive filesystem evidence.
    Ok(if observed {
        PathCase::Insensitive
    } else {
        PathCase::Sensitive
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_case_is_observed_on_the_selected_filesystem() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Workspace");
        std::fs::create_dir(&path).unwrap();
        let canonical = fs::canonical_context_path(&path).unwrap();
        let alternate = path.with_file_name("workspace");
        let insensitive = fs::path_file::same_file(&path, &alternate).unwrap_or(false);
        if !insensitive {
            assert_eq!(observe_case(&canonical).unwrap(), PathCase::Sensitive);
        }
        let expected = canonical_root(
            text(&canonical).unwrap(),
            native_style(),
            observe_case(&canonical).unwrap(),
        )
        .unwrap();
        assert_eq!(observe(&canonical).unwrap(), expected);
    }
    fn stack() -> bosn_core::manifest::Stack {
        let manifest = bosn_core::parse_manifest_toml(
            &format!("[stack.app]\nimage = 'example.invalid/app@sha256:{}'\n[stack.app.volumes.cache]\nscope = 'stack'\n[stack.app.volumes.scratch]\nscope = 'spec'\n", "a".repeat(64)),
            bosn_core::ManifestRoots::new("test", "/", "/"),
        ).unwrap();
        manifest.stack("app").unwrap().clone()
    }

    #[test]
    fn existing_linux_volume_identity_is_byte_identical() {
        let raw = "/home/user/project";
        let identity = canonical_root(raw, PathStyle::Posix, PathCase::Sensitive).unwrap();
        let volumes =
            crate::manifest_runtime::manifest_named_volumes(&stack(), raw, &identity, "sha256:abc")
                .unwrap();
        assert_eq!(volumes[0].name, "bosn-v-stack-c778b0d872ee4a4462eb7fda");
        assert_eq!(volumes[1].name, "bosn-v-spec-cf617e34354fa61b9049ef99");
        let legacy =
            crate::manifest_runtime::manifest_named_volumes(&stack(), raw, raw, "sha256:abc")
                .unwrap();
        assert_eq!(volumes, legacy);
    }

    #[test]
    fn windows_spellings_share_stack_and_spec_volume_hashes() {
        let stack = stack();
        let mut names = None;
        for raw in [
            r"C:\Dev\Foo",
            "C:/Dev/Foo",
            "/c/Dev/Foo",
            "/mnt/c/Dev/Foo",
            r"\\?\C:\Dev\Foo",
        ] {
            let identity = canonical_root(raw, PathStyle::Windows, PathCase::Insensitive).unwrap();
            let volumes = crate::manifest_runtime::manifest_named_volumes(
                &stack,
                raw,
                &identity,
                "sha256:abc",
            )
            .unwrap();
            let actual: Vec<_> = volumes.iter().map(|v| v.name.clone()).collect();
            assert!(
                volumes.iter().all(|v| v.workspace == raw),
                "preserve actual paths for I/O"
            );
            if let Some(expected) = &names {
                assert_eq!(&actual, expected);
            } else {
                names = Some(actual);
            }
        }
    }
    #[cfg(unix)]
    #[test]
    fn an_alternate_case_symlink_does_not_change_linux_identity() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Workspace");
        std::fs::create_dir(&path).unwrap();
        let alternate = path.with_file_name("workspace");
        if alternate.symlink_metadata().is_ok() {
            return;
        }
        std::os::unix::fs::symlink(&path, &alternate).unwrap();
        let canonical = fs::canonical_context_path(&path).unwrap();
        assert_eq!(observe_case(&canonical).unwrap(), PathCase::Sensitive);
        assert_eq!(observe(&canonical).unwrap(), canonical.to_str().unwrap());
    }
    #[test]
    fn windows_relative_inputs_reach_native_canonicalization() {
        for raw in [
            ".",
            "repo",
            "../Repo",
            r"\repo",
            r"\c",
            r"\c\repo",
            "C:relative",
        ] {
            assert_eq!(
                input_path(Path::new(raw), PathStyle::Windows).unwrap(),
                PathBuf::from(raw)
            );
        }
        assert_eq!(
            input_path(Path::new(r"\\?\C:\Dev\Foo"), PathStyle::Windows).unwrap(),
            PathBuf::from("C:/Dev/Foo")
        );
        assert!(input_path(Path::new(r"\\?\Volume{abc}\foo"), PathStyle::Windows).is_err());
    }
}
