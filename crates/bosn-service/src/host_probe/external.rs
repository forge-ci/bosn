//! Bounded container counts. Only Compose label values leave this probe.

use crate::capacity_observation::{External, LabelCount, Reason};
use bosn_engine::{CommandError, DockerEngine, RunOptions};
use serde::Deserialize;
use std::{collections::BTreeMap, time::Duration};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Container {
    state: State,
    labels: String,
    project: String,
    service: String,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum State {
    Created,
    Restarting,
    Running,
    Removing,
    Paused,
    Exited,
    Dead,
}

pub(crate) async fn read(deadline: Duration) -> Result<External, Reason> {
    // No names, images, commands, inspect, stats or info. ps is the sole command.
    let format = concat!(
        r#"{"state":{{json .State}},"labels":{{json .Labels}},"#,
        r#""project":{{json (.Label "com.docker.compose.project")}},"#,
        r#""service":{{json (.Label "com.docker.compose.service")}}}"#,
    );
    let result = DockerEngine::docker()
        .with_args(["ps", "-a", "--format", format])
        .capture_async(RunOptions::bounded(
            deadline.min(super::DOCKER_DEADLINE),
            super::DOCKER_OUTPUT_LIMIT,
        ))
        .await
        .map_err(command_reason)?;
    if !result.ok() {
        return Err(Reason::ParseError);
    }
    let text = std::str::from_utf8(&result.stdout).map_err(|_| Reason::ParseError)?;
    parse(text)
}

fn command_reason(error: CommandError) -> Reason {
    match error {
        CommandError::Deadline { .. } => Reason::Timeout,
        CommandError::Spawn(error) | CommandError::Io(error) => Reason::from_io(&error),
        _ => Reason::ParseError,
    }
}

pub(crate) fn parse(text: &str) -> Result<External, Reason> {
    let containers = text
        .lines()
        .map(serde_json::from_str::<Container>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| Reason::ParseError)?;
    let mut counts = BTreeMap::new();
    let mut other = 0;
    let mut total = 0;
    for container in containers {
        if container.state != State::Running
            || super::bosn_label_entries(&container.labels)
                .next()
                .is_some()
        {
            continue;
        }
        total += 1;
        let label = if !container.project.is_empty() {
            Some(("com.docker.compose.project", container.project))
        } else if !container.service.is_empty() {
            Some(("com.docker.compose.service", container.service))
        } else {
            None
        };
        match label {
            Some(label) => *counts.entry(label).or_insert(0usize) += 1,
            None => other += 1,
        }
    }
    let mut labels: Vec<_> = counts.into_iter().collect();
    labels.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let keep = if other > 0 || labels.len() > 20 {
        19
    } else {
        20
    };
    let mut by_label = Vec::new();
    for (index, ((key, value), count)) in labels.into_iter().enumerate() {
        if index < keep {
            by_label.push(LabelCount {
                key: key.into(),
                value,
                count: Some(count),
            });
        } else {
            other += count;
        }
    }
    if other > 0 {
        by_label.push(LabelCount {
            key: "other".into(),
            value: "other".into(),
            count: Some(other),
        });
    }
    Ok(External {
        containers_running_outside_bosn: Some(total),
        by_label: Some(by_label),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(project: &str, labels: &str) -> String {
        serde_json::json!({"state":"running","labels":labels,"project":project,"service":""})
            .to_string()
    }

    #[test]
    fn empty_registry_does_not_hide_unlabelled_or_orphaned_compose_containers() {
        let listing = [row("", ""), row("", "note=private-secret"), row("foo", "")].join("\n");
        let result = parse(&listing).unwrap();
        assert_eq!(result.containers_running_outside_bosn, Some(3));
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("foo"));
        assert!(!json.contains("private-secret"));
        assert_eq!(
            parse(&(listing + "\ninvalid")).unwrap_err(),
            Reason::ParseError
        );
        assert_eq!(parse("").unwrap().containers_running_outside_bosn, Some(0));
    }

    #[test]
    fn only_bosn_label_keys_exclude_containers_and_aggregation_is_bounded() {
        assert_eq!(
            parse(&row("", "com.zackees.bosn.stack=act"))
                .unwrap()
                .containers_running_outside_bosn,
            Some(0)
        );
        let listing = (0..25)
            .map(|i| row(&format!("project{i}"), ""))
            .collect::<Vec<_>>()
            .join("\n");
        let result = parse(&listing).unwrap();
        let labels = result.by_label.unwrap();
        assert_eq!(labels.len(), 20);
        assert_eq!(labels.iter().map(|l| l.count.unwrap()).sum::<usize>(), 25);
        assert_eq!(labels.last().unwrap().value, "other");
    }

    #[test]
    fn an_unknown_state_refuses_the_entire_listing() {
        let listing = format!(
            "{}\n{}",
            row("foo", ""),
            row("", "").replace("running", "garbage")
        );
        assert_eq!(parse(&listing).unwrap_err(), Reason::ParseError);
    }
}
