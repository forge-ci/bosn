//! Fixed-capacity, independently aggregated history tiers.
use super::Sample;
use serde::{Deserialize, Serialize};

pub const TIERS: [(u64, usize); 4] = [
    (10_000, 360),
    (60_000, 1440),
    (600_000, 1008),
    (3_600_000, 744),
];
pub const METRICS: [&str; 6] = [
    "cpu_percent",
    "io_delay_percent",
    "load1",
    "mem_used_bytes",
    "swap_used_bytes",
    "disk_used_bytes",
];

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Range {
    #[default]
    Hour,
    Day,
    Week,
    Month,
}
impl Range {
    pub fn tier(self) -> usize {
        match self {
            Self::Hour => 0,
            Self::Day => 1,
            Self::Week => 2,
            Self::Month => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Metric {
    pub avg: Option<f64>,
    pub max: Option<f64>,
    pub n: u64,
}
impl Metric {
    fn add(&mut self, value: Option<f64>) {
        if let Some(value) = value {
            self.n += 1;
            self.avg =
                Some(self.avg.unwrap_or(0.0) + (value - self.avg.unwrap_or(0.0)) / self.n as f64);
            self.max = Some(self.max.map_or(value, |old| old.max(value)));
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bucket {
    pub at_ms: u64,
    pub n: u64,
    pub metrics: [Metric; 6],
}
impl Bucket {
    pub fn empty(at_ms: u64) -> Self {
        Self {
            at_ms,
            n: 0,
            metrics: [Metric::default(); 6],
        }
    }
    fn add(&mut self, sample: &Sample) {
        self.n += 1;
        for (metric, value) in self.metrics.iter_mut().zip([
            sample.cpu_percent,
            sample.io_delay_percent,
            sample.load1,
            sample.mem_used_bytes.map(|n| n as f64),
            sample.swap_used_bytes.map(|n| n as f64),
            sample.disk_used_bytes.map(|n| n as f64),
        ]) {
            metric.add(value);
        }
    }
    pub fn point(&self) -> Point {
        Point {
            at_ms: self.at_ms,
            avg: Values::from_array(self.metrics.map(|m| m.avg)),
            max: Values::from_array(self.metrics.map(|m| m.max)),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Values {
    pub cpu_percent: Option<f64>,
    pub io_delay_percent: Option<f64>,
    pub load1: Option<f64>,
    pub mem_used_bytes: Option<f64>,
    pub swap_used_bytes: Option<f64>,
    pub disk_used_bytes: Option<f64>,
}
impl Values {
    fn from_array(v: [Option<f64>; 6]) -> Self {
        Self {
            cpu_percent: v[0],
            io_delay_percent: v[1],
            load1: v[2],
            mem_used_bytes: v[3],
            swap_used_bytes: v[4],
            disk_used_bytes: v[5],
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Point {
    pub at_ms: u64,
    pub avg: Values,
    pub max: Values,
}

#[derive(Clone)]
pub struct History {
    pub tiers: [Vec<Option<Bucket>>; 4],
}
impl Default for History {
    fn default() -> Self {
        Self {
            tiers: TIERS.map(|(_, capacity)| vec![None; capacity]),
        }
    }
}
impl History {
    pub fn record(&mut self, sample: &Sample) {
        for (index, (tier, (step, capacity))) in self.tiers.iter_mut().zip(TIERS).enumerate() {
            let at = sample.at_ms / step * step;
            let slot = (at / step % capacity as u64) as usize;
            let bucket = tier[slot].get_or_insert_with(|| Bucket::empty(at));
            if index == 0 || bucket.at_ms != at {
                *bucket = Bucket::empty(at);
            }
            bucket.add(sample);
        }
    }
    pub fn points(&self, range: Range, now: u64) -> Vec<Point> {
        let tier = range.tier();
        let (step, capacity) = TIERS[tier];
        let end = now / step * step;
        let start = end.saturating_sub(step * (capacity as u64 - 1));
        (start..=end)
            .step_by(step as usize)
            .map(|at| {
                self.tiers[tier][(at / step % capacity as u64) as usize]
                    .as_ref()
                    .filter(|b| b.at_ms == at)
                    .cloned()
                    .unwrap_or_else(|| Bucket::empty(at))
                    .point()
            })
            .collect()
    }
    pub fn load(&mut self, tier: usize, bucket: Bucket, now: u64) {
        let (step, capacity) = TIERS[tier];
        if bucket.at_ms <= now && now - bucket.at_ms < step * capacity as u64 {
            let slot = (bucket.at_ms / step % capacity as u64) as usize;
            self.tiers[tier][slot] = Some(bucket);
        }
    }
}

#[cfg(test)]
mod tests;
