use super::*;

fn checkout() -> Checkout {
    Checkout {
        repo: "owner/repo".into(),
        branch: "feature".into(),
        head: "a".repeat(40),
    }
}

fn merged(head: &str) -> String {
    format!(r#"[{{"number":732,"headRefOid":"{head}"}}]"#)
}

#[test]
fn success_cache_hit_and_head_mismatch() {
    let mut cache = Cache::new();
    let mut remaining = 8;
    let now = Instant::now();
    let mut branch = checkout();
    assert!(merged_with(
        &branch,
        &mut cache,
        &mut remaining,
        now,
        |args| {
            assert!(args.contains(&"number,headRefOid"));
            Ok(merged(&branch.head))
        }
    ));
    assert!(merged_with(
        &branch,
        &mut cache,
        &mut remaining,
        now,
        |_| panic!("cached")
    ));
    branch.head = "b".repeat(40);
    assert!(!merged_with(
        &branch,
        &mut cache,
        &mut remaining,
        now,
        |_| panic!("cached oid")
    ));
    assert_eq!(remaining, 7);
}

#[test]
fn missing_gh_timeout_and_bad_json_are_negative_and_expire() {
    for output in [
        Err(io::Error::from(io::ErrorKind::NotFound)),
        Err(io::Error::from(io::ErrorKind::TimedOut)),
        Ok("bad json".into()),
        Ok("[]".into()),
    ] {
        let mut cache = Cache::new();
        let mut remaining = 8;
        let now = Instant::now();
        let branch = checkout();
        assert!(!merged_with(
            &branch,
            &mut cache,
            &mut remaining,
            now,
            |_| output
        ));
        assert!(!merged_with(
            &branch,
            &mut cache,
            &mut remaining,
            now,
            |_| panic!("negative cache")
        ));
        assert!(merged_with(
            &branch,
            &mut cache,
            &mut remaining,
            now + NEGATIVE_TTL,
            |_| Ok(merged(&branch.head))
        ));
    }
}

#[test]
fn detached_and_default_branches_never_lookup_a_pr() {
    assert!(
        checkout_with(|_| Err(io::Error::from(io::ErrorKind::NotFound)))
            .unwrap()
            .is_none()
    );
    for branch in ["main", "master", "trunk"] {
        assert!(
            checkout_with(|args| match args {
                ["symbolic-ref", "--quiet", "--short", "HEAD"] => Ok(branch.into()),
                [
                    "symbolic-ref",
                    "--quiet",
                    "--short",
                    "refs/remotes/origin/HEAD",
                ] => Ok("origin/trunk".into()),
                _ => panic!("default branch must stop observation"),
            })
            .unwrap()
            .is_none()
        );
    }
}

#[test]
fn checkout_reads_origin_and_head_and_rejects_uncertainty() {
    let read = |args: &[&str]| {
        Ok(match args {
            ["symbolic-ref", "--quiet", "--short", "HEAD"] => "feature".into(),
            [
                "symbolic-ref",
                "--quiet",
                "--short",
                "refs/remotes/origin/HEAD",
            ] => "origin/main".into(),
            ["remote", "get-url", "origin"] => "git@github.com:owner/repo.git".into(),
            ["rev-parse", "--verify", "HEAD"] => "a".repeat(40),
            _ => panic!("unexpected git read"),
        })
    };
    let branch = checkout_with(read).unwrap().unwrap();
    assert_eq!(branch.repo, "owner/repo");
    assert_eq!(branch.head, "a".repeat(40));
    for origin in [
        "https://example.org/a/b",
        "https://github.com/a/b/extra",
        "https://github.com/a/b?query",
    ] {
        assert!(github_repo(origin).is_none());
    }
}

#[test]
fn lookup_budget_and_fresh_mismatch_keep_volumes() {
    let branch = checkout();
    assert!(!merged_with(
        &branch,
        &mut Cache::new(),
        &mut 0,
        Instant::now(),
        |_| panic!("budget")
    ));
    assert!(!merged_with(
        &branch,
        &mut Cache::new(),
        &mut 1,
        Instant::now(),
        |_| Ok(merged(&"b".repeat(40)))
    ));
}

#[test]
fn recheck_uses_only_cached_proof_for_the_exact_current_head() {
    let mut cache = Cache::new();
    let now = Instant::now();
    let mut branch = checkout();
    let mut budget = 1;
    assert!(merged_with(&branch, &mut cache, &mut budget, now, |_| Ok(
        merged(&branch.head)
    )));
    assert_eq!(budget, 0);
    assert!(merged_with(
        &branch,
        &mut cache,
        &mut budget,
        now,
        |_| panic!("no recheck lookup")
    ));
    branch.head = "b".repeat(40);
    assert!(!merged_with(
        &branch,
        &mut cache,
        &mut budget,
        now,
        |_| panic!("no recheck lookup")
    ));
    branch.branch = "other".into();
    assert!(!merged_with(
        &branch,
        &mut cache,
        &mut budget,
        now,
        |_| panic!("no recheck lookup")
    ));
}
