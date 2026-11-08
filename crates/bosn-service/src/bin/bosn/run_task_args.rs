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
) -> Result<bool, String> {
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
