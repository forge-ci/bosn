pub(super) fn collect_args(
    arguments: impl Iterator<Item = std::ffi::OsString>,
) -> Result<Vec<String>, String> {
    arguments
        .map(|arg| {
            arg.into_string()
                .map_err(|_| "task arguments must be UTF-8".to_owned())
        })
        .collect()
}

use std::path::Path;

pub(super) fn validate_task_args(
    text: &str,
    workspace: &Path,
    task: Option<&str>,
    args: &[String],
) -> Result<(), String> {
    if let Some(task) = task {
        let roots = bosn_core::manifest::ManifestRoots::new(
            "bosn.toml",
            workspace.to_string_lossy(),
            workspace.to_string_lossy(),
        );
        bosn_core::manifest::parse_manifest_toml(text, roots)
            .map_err(|e| e.to_string())?
            .task(task)
            .map_err(|e| e.to_string())?
            .validate_args(args)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(super) fn set_switch(
    flag: &str,
    no_ensure: &mut bool,
    detach: &mut bool,
    clean: &mut bosn_service::CleanVolumes,
) -> Result<bool, String> {
    if flag.starts_with("--clean") {
        if *clean != bosn_service::CleanVolumes::None {
            return Err("--clean given twice".into());
        }
        *clean = match flag {
            "--clean" => bosn_service::CleanVolumes::Checkout,
            "--clean=all" => bosn_service::CleanVolumes::All,
            _ => return Err("expected --clean or --clean=all".into()),
        };
        return Ok(true);
    }
    let switch = match flag {
        "--no-ensure" => no_ensure,
        "--detach" => detach,
        _ => return Ok(false),
    };
    if std::mem::replace(switch, true) {
        return Err(format!("{flag} given twice"));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    #[test]
    fn separator_preserves_args_and_requires_a_task() {
        let parse = |args: &[&str]| {
            super::super::parse_arguments(args.iter().map(std::ffi::OsString::from))
        };
        assert_eq!(
            parse(&["--task", "check", "--", "a", "b c", "--detach"])
                .unwrap()
                .args,
            ["a", "b c", "--detach"]
        );
        assert!(parse(&["--stack", "app", "--", "a"]).is_err());
    }

    #[test]
    fn manifest_refuses_args_before_submission() {
        let text = "[stack.app]\nimage = 'alpine:3'\n[task.check]\ncmd = 'echo'\nargs = false\n";
        let error = super::validate_task_args(
            text,
            std::path::Path::new("/tmp"),
            Some("check"),
            &["a".into()],
        )
        .unwrap_err();
        assert!(error.contains("refuses extra arguments"), "{error}");
        super::validate_task_args(text, std::path::Path::new("/tmp"), Some("check"), &[]).unwrap();
    }
}

#[cfg(test)]
mod clean_tests {
    #[test]
    fn clean_flags_require_a_task_and_accept_only_the_two_modes() {
        let parse = |args: &[&str]| {
            super::super::parse_arguments(args.iter().map(std::ffi::OsString::from))
        };
        assert!(parse(&["--task", "check", "--clean"]).is_ok());
        assert!(parse(&["--task", "check", "--clean=all"]).is_ok());
        assert!(parse(&["--stack", "app", "--clean"]).is_err());
        assert!(parse(&["--task", "check", "--clean=bad"]).is_err());
        assert!(parse(&["--task", "check", "--clean", "--clean=all"]).is_err());
    }
}
