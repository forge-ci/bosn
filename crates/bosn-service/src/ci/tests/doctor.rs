//! `bosn ci doctor` replies; it never deadlocks the daemon on its widget state.

use super::*;

#[test]
fn doctor_replies_instead_of_deadlocking_on_the_widget_state() {
    // A self-deadlock blocks the async worker itself, so no async timeout can
    // fire: bound the whole test from another OS thread instead.
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        with_registry(|registry, dir| async move {
            let backend = Arc::new(FakeBackend::with(Faults::default()));
            let runtime = CiRuntime::start(&dir, registry, backend, 1);
            // Twice: a guard leaked by the first reply would block the second.
            runtime.handle(CiRequest::Doctor).await.unwrap();
            runtime.handle(CiRequest::Doctor).await.unwrap();
        });
        let _ = done.send(());
    });
    finished
        .recv_timeout(Duration::from_secs(30))
        .expect("doctor must reply twice, not deadlock on the widget state");
}
