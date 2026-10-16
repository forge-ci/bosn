//! Typed mount and volume modes for manifest stacks (#666).

use crate::Scope;

/// How a `[stack.X.mounts.NAME]` bind exposes its source.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum MountMode {
    /// Bind read-write (the default).
    Rw,
    /// Bind read-only; `readonly = true` is an alias.
    Ro,
    /// Read-only source as the lower layer under a writable upper layer.
    Overlay,
}

impl MountMode {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "rw" => Self::Rw,
            "ro" => Self::Ro,
            "overlay" => Self::Overlay,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rw => "rw",
            Self::Ro => "ro",
            Self::Overlay => "overlay",
        }
    }
}

/// Where an overlay mount's writes land.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum OverlayUpper {
    /// Discarded when the container ends.
    Tmpfs,
    /// Kept in the stack's declared volume of this name.
    Volume(String),
}

impl OverlayUpper {
    pub fn parse(value: &str) -> Self {
        if value == "tmpfs" {
            Self::Tmpfs
        } else {
            Self::Volume(value.to_owned())
        }
    }
}

/// Whether runs of different names may mount a stack volume at once.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum VolumeMode {
    /// One run at a time.
    Exclusive,
    /// Any number of concurrent runs; the tools inside lock for themselves.
    Shared,
}

impl VolumeMode {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "exclusive" => Self::Exclusive,
            "shared" => Self::Shared,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exclusive => "exclusive",
            Self::Shared => "shared",
        }
    }
    /// `shared` for machine volumes, `exclusive` for stack and spec volumes.
    pub fn default_for(scope: Scope) -> Self {
        match scope {
            Scope::Machine => Self::Shared,
            Scope::Spec | Scope::Stack => Self::Exclusive,
        }
    }
}
