//! Pull-driven replay of persisted per-run status and raw log events.

use std::{collections::VecDeque, io, time::Duration};

use futures_util::stream;
use kernal_api::{async_engine, http_server};

use crate::ci::{
    CiRuntime,
    cursor::{self, Event},
    store::Store,
    wire::RunState,
};

struct Tail {
    store: Store,
    run: String,
    cursor: u64,
    offsets: [u64; 2],
    pending: VecDeque<Event>,
    done: bool,
    terminal_checked: bool,
}

pub(super) fn response(ci: &CiRuntime, run: &str, from_seq: u64) -> http_server::Response {
    if ci.record(run).is_err() {
        return super::text(404, "run not found");
    }
    let state = Tail {
        store: ci.status_store(),
        run: run.into(),
        cursor: from_seq,
        offsets: [0; 2],
        pending: VecDeque::new(),
        done: false,
        terminal_checked: false,
    };
    let runtime = ci.clone();
    let events = stream::unfold(state, move |mut tail| {
        let runtime = runtime.clone();
        async move {
            loop {
                if tail.done {
                    return None;
                }
                if let Some(snapshot) = tail.pending.pop_front() {
                    tail.cursor = snapshot.seq;
                    tail.done = snapshot.terminal;
                    let event = http_server::SseEvent {
                        id: Some(snapshot.seq.to_string()),
                        event: Some(snapshot.kind),
                        data: snapshot.data,
                    };
                    return Some((Ok(event), tail));
                }
                let (store, run, cursor, mut offsets) = (
                    tail.store.clone(),
                    tail.run.clone(),
                    tail.cursor,
                    tail.offsets,
                );
                let page = async_engine::launch_blocking(move || {
                    cursor::replay(&store, &run, cursor, &mut offsets)
                        .map(|records| (records, offsets))
                })
                .await;
                match page {
                    Ok(Ok((records, offsets))) => {
                        tail.offsets = offsets;
                        if !records.is_empty() {
                            tail.pending = records.into();
                            continue;
                        }
                        if tail.terminal_checked {
                            return None;
                        }
                        if let Ok(record) = runtime.record(&tail.run)
                            && record.state == RunState::Done
                        {
                            match runtime.reconcile_status(record.id).await {
                                // Re-read after reconciliation even when another writer
                                // appended terminal status after our last empty page.
                                Ok((_, RunState::Done)) => {
                                    tail.terminal_checked = true;
                                    continue;
                                }
                                Ok((true, _)) => continue,
                                Ok((false, _)) => {
                                    async_engine::sleep(Duration::from_millis(250)).await;
                                }
                                Err(error) => return Some((Err(error), tail)),
                            }
                        }
                        async_engine::sleep(Duration::from_millis(250)).await;
                    }
                    Ok(Err(error)) if error.kind() == io::ErrorKind::NotFound => {
                        async_engine::sleep(Duration::from_millis(250)).await;
                    }
                    Ok(Err(error)) => return Some((Err(error), tail)),
                    Err(error) => return Some((Err(io::Error::other(error)), tail)),
                }
            }
        }
    });
    http_server::Response::sse_stream(events, super::SSE_KEEPALIVE)
        .unwrap_or_else(|_| super::text(500, "status stream unavailable"))
}
