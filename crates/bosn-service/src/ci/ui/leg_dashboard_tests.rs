//! Matrix-runner fixture expectations, kept separate from needs/failure.
use super::*;

pub(super) const RECORDED: &str =
    include_str!("../../../tests/fixtures/act/act-0.2.88-matrix-legs-runner.jsonl");
pub(super) const LISTING: &str = "Stage  Job ID  Job name                  Workflow name  Workflow file  Events\n\
0      first   first                     legs           legs.yml       push\n\
1      lint    lint                      legs           legs.yml       push\n\
1      wheel   Wheel (${{ matrix.os }})  legs           legs.yml       push\n";

pub(super) fn assert_legs(view: &RunView) {
    let jobs: Vec<_> = view.record.tree.jobs().collect();
    assert_eq!(jobs.len(), 5);
    assert_eq!(view.record.conclusion, Some(Conclusion::Incomplete));
    for (name, conclusion, duration) in [
        ("legs/first", ItemConclusion::Success, 60),
        (
            "legs/Wheel (ubuntu-latest) (3.11)",
            ItemConclusion::Success,
            65,
        ),
        (
            "legs/Wheel (windows-latest) (3.11)",
            ItemConclusion::Unsupported,
            76,
        ),
        ("legs/lint (ruff)", ItemConclusion::Success, 59),
        ("legs/lint (pyright)", ItemConclusion::Success, 60),
    ] {
        let job = jobs
            .iter()
            .find(|j| j.key == name)
            .unwrap_or_else(|| panic!("missing {name}: {jobs:#?}"));
        assert_eq!(job.conclusion, Some(conclusion));
        assert_eq!(job.sections.first().unwrap().name, "Set up job");
        assert_eq!(job.sections.last().unwrap().name, "Complete job");
        assert_eq!(job.sections[1].duration_ms, Some(duration));
        assert!(
            job.sections
                .iter()
                .all(|step| step.status == ItemStatus::Completed
                    && step.conclusion == Some(ItemConclusion::Success))
        );
    }
}
