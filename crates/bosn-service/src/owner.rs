//! The client process that owns a piece of daemon work.
//!
//! A client that follows its work (`bosn run`, `bosn ci run`, the MCP server)
//! hands the daemon its PID and the host's creation key for that PID. The
//! creation key tells the live client from a later process that reuses its
//! PID, and is what Windows needs to identify a process at all. The daemon
//! checks every owner on its sweep (one process-table lookup each) and
//! cancels the work of any owner that is gone, returning its slots and
//! containers at once instead of at the work's deadline. Work submitted with
//! `--detach` carries no owner and outlives its client.

use std::{
    collections::{BTreeMap, BTreeSet},
    hash::{Hash, Hasher},
};

use kernal_api::platform::process::{self, ProcessIdentity, ProcessIdentityCapture};

use crate::Error;

/// One live client process: its PID and the host's native creation key
/// (Linux `/proc` start ticks, the Windows `FILETIME`, the macOS start time).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ClientOwner {
    pid: u32,
    created: [u64; 2],
}

/// What one check of an owner found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Liveness {
    Alive,
    /// Exited, a zombie, or its PID now names another process.
    Gone,
    /// The host would not say (permission denied, an I/O error). Treated as
    /// alive: only proof of exit cancels work.
    Unknown,
}

impl ClientOwner {
    /// This process, or `None` when the host cannot identify it (its work is
    /// then unowned, as if detached).
    pub fn current() -> Option<Self> {
        Self::of_pid(std::process::id())
    }

    /// The live process `pid`, if the host identifies it.
    pub(crate) fn of_pid(pid: u32) -> Option<Self> {
        match process::capture_identity(pid) {
            ProcessIdentityCapture::Found(identity) => Self::of(identity),
            _ => None,
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The owner fields of a request: no owner is PID zero and no key.
    pub(crate) fn to_wire(owner: Option<Self>) -> (u32, Vec<u64>) {
        owner.map_or((0, Vec::new()), |o| (o.pid, o.created.to_vec()))
    }

    /// Parse a request's owner fields eagerly; PID zero means unowned.
    pub(crate) fn from_wire(pid: u32, created: &[u64]) -> Result<Option<Self>, Error> {
        match (pid, created) {
            (0, []) => Ok(None),
            (1.., &[a, b]) => Ok(Some(Self {
                pid,
                created: [a, b],
            })),
            _ => Err(Error::Protocol("invalid owner process identity")),
        }
    }

    /// Look this owner up in the process table once.
    pub fn liveness(&self) -> Liveness {
        match process::capture_identity(self.pid) {
            ProcessIdentityCapture::Exited => Liveness::Gone,
            ProcessIdentityCapture::Found(identity) => match Self::of(identity) {
                Some(current) if current != *self => Liveness::Gone,
                Some(_) if zombie(self.pid) => Liveness::Gone,
                Some(_) => Liveness::Alive,
                None => Liveness::Unknown,
            },
            ProcessIdentityCapture::Unavailable(_) | ProcessIdentityCapture::Error(_) => {
                Liveness::Unknown
            }
        }
    }

    /// kernal-api keeps the creation key opaque but hashable; its derived
    /// `Hash` writes the PID, the key's length and the key words, in that
    /// order. Reading them back is checked: any other layout yields `None`
    /// (and fails `the_current_process_has_an_owner_identity`).
    fn of(identity: ProcessIdentity) -> Option<Self> {
        let mut bytes = Recorder(Vec::new());
        identity.hash(&mut bytes);
        let bytes = bytes.0;
        let word = std::mem::size_of::<usize>();
        let (pid, rest) = bytes.split_first_chunk::<4>()?;
        if u32::from_ne_bytes(*pid) != identity.pid() || rest.len() != word + 16 {
            return None;
        }
        let (length, key) = rest.split_at(word);
        if length != 2usize.to_ne_bytes() {
            return None;
        }
        let (a, b) = key.split_at(8);
        Some(Self {
            pid: identity.pid(),
            created: [
                u64::from_ne_bytes(a.try_into().ok()?),
                u64::from_ne_bytes(b.try_into().ok()?),
            ],
        })
    }
}

/// Collects the bytes a `Hash` implementation writes.
struct Recorder(Vec<u8>);

impl Hasher for Recorder {
    fn write(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }
    fn finish(&self) -> u64 {
        0
    }
}

/// An exited child its parent has not reaped still has a `/proc` entry.
#[cfg(target_os = "linux")]
fn zombie(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.trim_start().chars().next())
            .is_some_and(|state| matches!(state, 'Z' | 'X'))
    })
}

#[cfg(not(target_os = "linux"))]
fn zombie(_pid: u32) -> bool {
    false
}

/// The owners of each piece of unfinished work, keyed by job or run ID.
///
/// Work created by an owned submission is owned; an identical submission that
/// joins it adds its owner, so shared work lives while any owner does. A
/// joiner without an owner (`--detach`, an older client) pins the work, and
/// work created unowned never gains an owner: joining another caller's work
/// must not make it depend on this caller staying alive.
#[derive(Debug)]
pub(crate) struct Owners<K> {
    owned: BTreeMap<K, BTreeSet<ClientOwner>>,
}

impl<K> Default for Owners<K> {
    fn default() -> Self {
        Self {
            owned: BTreeMap::new(),
        }
    }
}

impl<K: Ord + Clone> Owners<K> {
    /// Record one submission that created (`created`) or joined `key`.
    pub(crate) fn submitted(&mut self, key: &K, owner: Option<ClientOwner>, created: bool) {
        match (owner, created) {
            (Some(owner), true) => {
                self.owned.insert(key.clone(), BTreeSet::from([owner]));
            }
            (Some(owner), false) => {
                if let Some(owners) = self.owned.get_mut(key) {
                    owners.insert(owner);
                }
            }
            (None, _) => {
                self.owned.remove(key);
            }
        }
    }

    /// `owner` lets go of `key` because its client is exiting. True when it
    /// was the last owner, so the work should be cancelled now; detached or
    /// unowned work, and work another owner still holds, keep running.
    pub(crate) fn release(&mut self, key: &K, owner: &ClientOwner) -> bool {
        let Some(owners) = self.owned.get_mut(key) else {
            return false;
        };
        if !owners.remove(owner) || !owners.is_empty() {
            return false;
        }
        self.owned.remove(key);
        true
    }

    /// The work ended; forget its owners.
    pub(crate) fn forget(&mut self, key: &K) {
        self.owned.remove(key);
    }

    /// Remove and return the work whose every owner is gone, each with its
    /// owners' PIDs. Each distinct owner is looked up once.
    pub(crate) fn orphaned(
        &mut self,
        mut liveness: impl FnMut(&ClientOwner) -> Liveness,
    ) -> Vec<(K, Vec<u32>)> {
        let mut seen = BTreeMap::new();
        let mut gone = |owner: &ClientOwner| {
            *seen
                .entry(*owner)
                .or_insert_with(|| liveness(owner) == Liveness::Gone)
        };
        let orphaned: Vec<K> = self
            .owned
            .iter()
            .filter(|(_, owners)| owners.iter().all(&mut gone))
            .map(|(key, _)| key.clone())
            .collect();
        orphaned
            .into_iter()
            .filter_map(|key| {
                let owners = self.owned.remove(&key)?;
                Some((key, owners.iter().map(ClientOwner::pid).collect()))
            })
            .collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.owned.is_empty()
    }
}

/// The log line for work cancelled because its owners are gone.
pub(crate) fn orphaned_message(pids: &[u32]) -> String {
    let pids = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "cancelled: the client that started this (pid {pids}) exited; use --detach to keep work running after its client"
    )
}

#[cfg(test)]
mod tests;
