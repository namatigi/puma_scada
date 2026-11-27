//! Coalesces tag addresses into as few Modbus read requests as possible.
//!
//! Tags on the same driver, station and area are sorted by register and merged
//! into one request while the gap between them is at most `max_gap` registers and
//! the request stays within the protocol limit (125 registers / 2000 bits).
//! Reading a few unused registers is cheaper than an extra round trip.

use moviclone_model::{Address, ModbusArea, Project};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PollJob {
    pub driver: String,
    pub station: String,
    pub area: ModbusArea,
    /// First register (as configured, before one-based adjustment).
    pub start: u16,
    /// Registers or bits read.
    pub count: u16,
    pub tags: Vec<String>,
}

/// (driver, station, area) → (register, length, tag name) for each tag.
type Groups<'a> = BTreeMap<(&'a str, &'a str, ModbusArea), Vec<(u16, u16, &'a str)>>;

pub fn plan_polls(p: &Project, max_gap: u16) -> Vec<PollJob> {
    let mut groups: Groups = BTreeMap::new();
    for t in p.tags.iter().filter(|t| t.access.reads()) {
        if let Some(Address::Modbus(a)) = &t.address {
            let len = t.data_type.registers().unwrap_or(1);
            groups
                .entry((a.driver.as_str(), a.station.as_str(), a.area))
                .or_default()
                .push((a.register, len, t.name.as_str()));
        }
    }

    let mut jobs = Vec::new();
    for ((driver, station, area), mut spans) in groups {
        spans.sort();
        let mut current: Option<PollJob> = None;
        for (start, len, name) in spans {
            let end = start as u32 + len as u32; // exclusive
            if let Some(job) = current.as_mut() {
                let job_end = job.start as u32 + job.count as u32;
                let fits = start as u32 <= job_end + max_gap as u32
                    && end.max(job_end) - job.start as u32 <= area.max_read() as u32;
                if fits {
                    job.count = (end.max(job_end) - job.start as u32) as u16;
                    job.tags.push(name.to_string());
                    continue;
                }
                jobs.push(current.take().unwrap());
            }
            current = Some(PollJob {
                driver: driver.to_string(),
                station: station.to_string(),
                area,
                start,
                count: len,
                tags: vec![name.to_string()],
            });
        }
        jobs.extend(current);
    }
    jobs
}
