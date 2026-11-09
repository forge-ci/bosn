//! Best-effort merged-PR observations (#732). Called only by the blocking retention pass.
use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use bosn_core::retention::EagerFacts;
use kernal_api::{SpawnSpec, StreamMode};
use serde::Deserialize;

const NEGATIVE_TTL: Duration = Duration::from_secs(3600);
const LOOKUPS_PER_PASS: usize = 8;
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
static FAILURE_LOGGED: AtomicBool = AtomicBool::new(false);

type Cache = BTreeMap<(String, String), Answer>;
struct Answer {
    head: Option<String>,
    observed: Instant,
}

#[derive(Debug)]
struct Checkout {
    repo: String,
    branch: String,
    head: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergedPr {
    number: u64,
    head_ref_oid: String,
}

fn log_failure() {
    if !FAILURE_LOGGED.swap(true, Ordering::Relaxed) {
        crate::daemon_log!(
            "bosn retention: merged-branch observation unavailable; keeping volumes without a signal"
        );
    }
}

fn command(program: &str, directory: &Path, args: &[&str]) -> io::Result<String> {
    let spec = SpawnSpec::new(program)
        .current_dir(directory)
        .stdin(StreamMode::Null)
        .stdout(StreamMode::Piped)
        .stderr(StreamMode::Piped)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GH_PROMPT_DISABLED", "1");
    let spec = args.iter().fold(spec, |spec, arg| spec.arg(*arg));
    let output = kernal_api::run_bounded_command(spec, Duration::from_secs(3), 16384)
        .map_err(io::Error::other)?;
    if output.exit.raw_code() != 0 {
        return Err(io::Error::other("unavailable or invalid observation"));
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim().to_owned())
        .map_err(io::Error::other)
}

fn github_repo(origin: &str) -> Option<String> {
    let path = origin
        .strip_prefix("git@github.com:")
        .or_else(|| origin.strip_prefix("https://github.com/"))
        .or_else(|| origin.strip_prefix("ssh://git@github.com/"))?;
    let path = path.strip_suffix(".git").unwrap_or(path);
    let parts: Vec<_> = path.split('/').collect();
    (parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        }))
    .then(|| path.to_owned())
}

fn checkout_with(
    mut git: impl FnMut(&[&str]) -> io::Result<String>,
) -> io::Result<Option<Checkout>> {
    // symbolic-ref fails on detached HEAD: no branch means no signal.
    let Ok(branch) = git(&["symbolic-ref", "--quiet", "--short", "HEAD"]) else {
        return Ok(None);
    };
    if branch.is_empty() || matches!(branch.as_str(), "main" | "master") {
        return Ok(None);
    }
    // Missing origin/HEAD is unknown, so keep rather than guess a custom default branch.
    let default = git(&[
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ])?;
    if default.strip_prefix("origin/") == Some(branch.as_str()) {
        return Ok(None);
    }
    let origin = git(&["remote", "get-url", "origin"])?;
    let repo = github_repo(&origin).ok_or_else(|| io::Error::other("non-GitHub origin"))?;
    let head = git(&["rev-parse", "--verify", "HEAD"])?;
    if !valid_oid(&head) {
        return Err(io::Error::other("unavailable or invalid observation"));
    }
    Ok(Some(Checkout { repo, branch, head }))
}

fn valid_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64) && oid.bytes().all(|c| c.is_ascii_hexdigit())
}

fn merged_with(
    checkout: &Checkout,
    cache: &mut Cache,
    remaining: &mut usize,
    now: Instant,
    gh: impl FnOnce(&[&str]) -> io::Result<String>,
) -> bool {
    let key = (checkout.repo.clone(), checkout.branch.clone());
    if let Some(answer) = cache.get(&key)
        && (answer.head.is_some() || now.duration_since(answer.observed) < NEGATIVE_TTL)
    {
        return answer.head.as_ref() == Some(&checkout.head);
    }
    if *remaining == 0 {
        return false;
    }
    *remaining -= 1;
    let result = gh(&[
        "pr",
        "list",
        "--repo",
        &checkout.repo,
        "--head",
        &checkout.branch,
        "--state",
        "merged",
        "--json",
        "number,headRefOid",
        "--limit",
        "1",
    ])
    .and_then(|text| serde_json::from_str::<Vec<MergedPr>>(&text).map_err(io::Error::other));
    let head = match result {
        Ok(prs) if prs.len() <= 1 => prs
            .into_iter()
            .next()
            .filter(|pr| pr.number > 0 && valid_oid(&pr.head_ref_oid))
            .map(|pr| pr.head_ref_oid),
        _ => {
            log_failure();
            None
        }
    };
    let merged = head.as_ref() == Some(&checkout.head);
    cache.insert(
        key,
        Answer {
            head,
            observed: now,
        },
    );
    merged
}

pub(super) fn observe(facts: &mut EagerFacts) {
    let Ok(mut cache) = CACHE.get_or_init(|| Mutex::new(Cache::new())).lock() else {
        log_failure();
        return;
    };
    let mut remaining = LOOKUPS_PER_PASS;
    // Families were read by #659's common-dir reader: only present git workspaces qualify.
    for workspace in facts.families.keys() {
        let directory = Path::new(workspace);
        match checkout_with(|args| command("git", directory, args)) {
            Ok(Some(checkout)) => {
                if merged_with(
                    &checkout,
                    &mut cache,
                    &mut remaining,
                    Instant::now(),
                    |args| command("gh", directory, args),
                ) {
                    facts.merged_workspaces.insert(workspace.clone());
                }
            }
            Ok(None) => {}
            Err(_) => log_failure(),
        }
    }
}

#[cfg(test)]
mod tests;
