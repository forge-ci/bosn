//! Human and JSON rendering for `bosn ci doctor`.

use super::*;

/// `bosn ci doctor`: act2's `act doctor` report from inside the shared
/// engine, printed as act prints it, with its exit code.
pub(super) fn run(arguments: impl Iterator<Item = OsString>) -> Result<i32, Failure> {
    let flags = Flags::parse(arguments, &["--state-dir"], &["--json"])?;
    let (runtime, client) = connect(&flags)?;
    let reply = call(&runtime, client.ci_doctor())?;
    print(&reply, flags.json(), |r| {
        println!(
            "widget: {} (daemon {})",
            r.widget_release.as_deref().unwrap_or(
                if r.widget_presence == Some(bosn_service::ci::widget::WidgetPresence::Connected) {
                    "unknown"
                } else {
                    "absent"
                }
            ),
            r.daemon_release.as_deref().unwrap_or("unknown"),
        );
        println!("{}", format_task_capacity(r.task_capacity.as_ref()));
        println!(
            "{}",
            format_ci_capacity(r.admission_limit, r.engine_capacity.as_ref())
        );
        match (&r.engine, &r.report) {
            (Some(engine), Some(report)) => {
                println!("engine: {engine}");
                for check in &report.checks {
                    let need = if check.required {
                        "required"
                    } else {
                        "optional"
                    };
                    println!(
                        "{:<4} {:<19} {:<8} {}",
                        check.status.as_str(),
                        check.id,
                        need,
                        check.summary
                    );
                    if let Some(fix) = check.remediation.as_deref()
                        && check.status != bosn_service::ci::ActDoctorStatus::Pass
                    {
                        println!("     fix: {fix}");
                    }
                }
            }
            _ => println!("no shared CI engine is running; the next `bosn ci run` makes it"),
        }
    });
    Ok(match &reply.report {
        Some(report) if !report.ok => 1,
        _ => 0,
    })
}
