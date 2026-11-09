//! Human rendering reads typed localization facts; JSON remains the daemon reply.

use bosn_service::ci::{Conclusion, RunView, store::Store};

use super::{Failure, Flags};

pub(super) fn print_view(view: &RunView, flags: &Flags) -> Result<(), Failure> {
    if flags.json() {
        super::print(view, true, |_| {});
        return Ok(());
    }
    let notes = Store::new(&flags.state_dir())
        .load_notes(&view.record.id)
        .map_err(|error| Failure::error(format!("cannot read CI notes: {error}")))?;
    print_tree(view);
    let text = notes.render(
        &view.record.id,
        flags.has("--verbose") || flags.has("--notes"),
    );
    if !text.is_empty() {
        println!("{text}");
    }
    Ok(())
}

fn print_tree(view: &RunView) {
    let run = &view.record;
    println!(
        "run {} {} {}  sha {}{}  {}",
        run.id,
        run.display_state(),
        run.conclusion.map_or("-", Conclusion::as_str),
        run.sha,
        if run.dirty.is_some() { " +dirty" } else { "" },
        run.workflow,
    );
    println!("  trigger: {} ({})", run.trigger.as_str(), run.event);
    if let Some(params) = run.params.describe() {
        println!("  params: {params}");
    }
    if let Some(reason) = &run.reason {
        println!("  reason: {reason}");
    }
    super::print_setup_stages(&run.stages);
    for group in &run.tree.groups {
        println!("  stage {}", group.name);
        for job in &group.jobs {
            println!(
                "    {} {}",
                super::mark(job.conclusion, job.status),
                job.key
            );
            if job.conclusion == Some(super::ItemConclusion::RemoteOnly) {
                continue;
            }
            for section in &job.sections {
                println!(
                    "      {} {} {}",
                    super::mark(section.conclusion, section.status),
                    section.stage,
                    section.name
                );
            }
        }
    }
}
