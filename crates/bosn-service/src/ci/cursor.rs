//! One HTTP event order, stored additively alongside the existing journal counters.
use super::{
    RunState,
    model::LogRecord,
    status::{self, StatusSnapshot},
    store::Store,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs::File,
    io::{self, BufRead, Seek},
    path::Path,
};

// A numeric discriminator, exactly representable by JavaScript. Lower cursors
// retain the old status-only meaning; existing files are never rewritten.
pub(super) const BASE: u64 = 1 << 40;

#[derive(Serialize, Deserialize)]
pub(super) struct Stored<T> {
    #[serde(default)]
    pub event_seq: u64,
    #[serde(flatten)]
    pub record: T,
}

#[derive(Deserialize)]
struct CursorRow {
    #[serde(default)]
    event_seq: u64,
}

pub(super) fn append<T>(
    store: &Store,
    run: &str,
    write: impl FnOnce(u64) -> io::Result<T>,
) -> io::Result<T> {
    let mut cursors = store.cursors.lock().unwrap_or_else(|e| e.into_inner());
    let last = match cursors.get(run) {
        Some(last) => *last,
        None => {
            let mut last = BASE;
            for path in [status::path(store, run), store.log_path(run)] {
                let file = match File::open(path) {
                    Ok(file) => file,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                for line in io::BufReader::new(file).split(b'\n') {
                    if let Ok(row) = serde_json::from_slice::<CursorRow>(&line?) {
                        last = last.max(row.event_seq);
                    }
                }
            }
            last
        }
    };
    let next = last
        .checked_add(1)
        .ok_or_else(|| io::Error::other("event cursor exhausted"))?;
    // Reserve even on failure; no later append may reuse a partially written id.
    cursors.insert(run.into(), next);
    write(next)
}

pub(super) struct Event {
    pub seq: u64,
    pub kind: String,
    pub data: String,
    pub terminal: bool,
}

impl Event {
    fn status(row: Stored<StatusSnapshot>) -> io::Result<Self> {
        let mut record = row.record;
        record.seq = row.event_seq.max(record.seq);
        Ok(Self {
            seq: record.seq,
            kind: "status".into(),
            terminal: record.state == RunState::Done,
            data: serde_json::to_string(&record).map_err(io::Error::other)?,
        })
    }
    fn log(row: Stored<LogRecord>) -> io::Result<Self> {
        let mut record = row.record;
        record.seq = row.event_seq;
        Ok(Self {
            seq: record.seq,
            kind: record.stream.clone(),
            terminal: false,
            data: serde_json::to_string(&record).map_err(io::Error::other)?,
        })
    }
}

fn page<T: DeserializeOwned>(
    path: &Path,
    offset: &mut u64,
    accepts: impl Fn(&Stored<T>) -> bool,
    event: impl Fn(Stored<T>) -> io::Result<Event>,
) -> io::Result<Vec<(u64, Event)>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut reader = io::BufReader::new(file);
    reader.seek(io::SeekFrom::Start(*offset))?;
    let mut at = *offset;
    let mut records = Vec::new();
    loop {
        let mut line = Vec::new();
        if reader.read_until(b'\n', &mut line)? == 0 || !line.ends_with(b"\n") {
            break;
        }
        at += line.len() as u64;
        let row: Stored<T> = serde_json::from_slice(&line).map_err(io::Error::other)?;
        if accepts(&row) {
            records.push((at, event(row)?));
            if records.len() == 64 {
                break;
            }
        } else if records.is_empty() {
            *offset = at;
        }
    }
    Ok(records)
}

pub(super) fn replay(
    store: &Store,
    run: &str,
    since: u64,
    offsets: &mut [u64; 2],
) -> io::Result<Vec<Event>> {
    // Readers and both writers share this lock, so no lower reserved id can
    // appear after a reader has already delivered a higher one.
    let _guard = store.cursors.lock().unwrap_or_else(|e| e.into_inner());
    let statuses = page(
        &status::path(store, run),
        &mut offsets[0],
        |row: &Stored<StatusSnapshot>| {
            if since < BASE {
                row.record.seq > since
            } else {
                row.event_seq > since
            }
        },
        Event::status,
    )?;
    let logs = page(
        &store.log_path(run),
        &mut offsets[1],
        |row: &Stored<LogRecord>| row.event_seq > since.max(BASE),
        Event::log,
    )?;
    let mut merged: Vec<_> = statuses
        .into_iter()
        .map(|(at, event)| (0, at, event))
        .chain(logs.into_iter().map(|(at, event)| (1, at, event)))
        .collect();
    merged.sort_by_key(|(_, _, event)| event.seq);
    Ok(merged
        .into_iter()
        .take(64)
        .map(|(kind, at, event)| {
            offsets[kind] = at;
            event
        })
        .collect())
}
