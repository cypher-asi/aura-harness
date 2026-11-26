use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The machine sizes boxd offers: vCPUs and memory move together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub enum MachineSize {
    /// 1 vCPU, 4 GiB.
    S1x4,
    /// 2 vCPU, 8 GiB (boxd's own default).
    S2x8,
    /// 4 vCPU, 16 GiB: four parallel Rust compile jobs fit without swap.
    #[default]
    S4x16,
    /// 8 vCPU, 32 GiB, where the org quota allows it.
    S8x32,
}

impl MachineSize {
    /// Every size, smallest first.
    pub const ALL: [Self; 4] = [Self::S1x4, Self::S2x8, Self::S4x16, Self::S8x32];

    /// vCPUs of the size.
    #[must_use]
    pub const fn vcpu(self) -> u32 {
        match self {
            Self::S1x4 => 1,
            Self::S2x8 => 2,
            Self::S4x16 => 4,
            Self::S8x32 => 8,
        }
    }

    /// Memory of the size, in GiB.
    #[must_use]
    pub const fn memory_gib(self) -> u32 {
        self.vcpu() * 4
    }

    /// Memory of the size, in bytes.
    #[must_use]
    pub const fn memory_bytes(self) -> u64 {
        self.memory_gib() as u64 * 1024 * 1024 * 1024
    }

    /// The size as boxd's machine specs name it (`"4x16"`).
    #[must_use]
    pub const fn as_spec(self) -> &'static str {
        match self {
            Self::S1x4 => "1x4",
            Self::S2x8 => "2x8",
            Self::S4x16 => "4x16",
            Self::S8x32 => "8x32",
        }
    }

    /// The next size down, for falling back when a quota refuses this one.
    #[must_use]
    pub const fn smaller(self) -> Option<Self> {
        match self {
            Self::S1x4 => None,
            Self::S2x8 => Some(Self::S1x4),
            Self::S4x16 => Some(Self::S2x8),
            Self::S8x32 => Some(Self::S4x16),
        }
    }

    /// The size with `vcpu` vCPUs, if boxd offers one.
    #[must_use]
    pub fn from_vcpu(vcpu: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|size| size.vcpu() == vcpu)
    }
}

impl fmt::Display for MachineSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_spec())
    }
}

/// Failure to parse a [`MachineSize`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown machine size `{0}`: expected 1x4, 2x8, 4x16 or 8x32")]
pub struct MachineSizeError(pub String);

impl FromStr for MachineSize {
    type Err = MachineSizeError;

    /// Accepts `4x16`, `4x16G`, `4x16GiB` and a bare vCPU count (`4`),
    /// case-insensitively.
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let lower = raw.trim().to_ascii_lowercase();
        let err = || MachineSizeError(raw.trim().to_string());
        let (vcpu, memory) = match lower.split_once('x') {
            Some((vcpu, memory)) => (vcpu, Some(memory)),
            None => (lower.as_str(), None),
        };
        let size = vcpu
            .trim()
            .parse::<u32>()
            .ok()
            .and_then(Self::from_vcpu)
            .ok_or_else(err)?;
        if let Some(memory) = memory {
            let digits = memory
                .trim()
                .trim_end_matches("gib")
                .trim_end_matches("gb")
                .trim_end_matches('g');
            let gib = digits.parse::<u32>().map_err(|_| err())?;
            if gib != size.memory_gib() {
                return Err(err());
            }
        }
        Ok(size)
    }
}

impl Serialize for MachineSize {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_spec())
    }
}

impl<'de> Deserialize<'de> for MachineSize {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}
