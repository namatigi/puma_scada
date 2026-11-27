//! Turns the project's poll plan into ready-to-send requests for one driver.

use moviclone_check::plan_polls;
use moviclone_model::{Addressing, DataType, Driver, DriverConfig, ModbusArea, ModbusStation, Project};
use moviclone_tagdb::{TagId, TagStore};
use std::collections::HashMap;

/// Largest gap of unused registers merged into one read. Zero matches
/// `movi build --plan` and never reads addresses no tag refers to, which some
/// devices answer with an exception.
pub const MAX_GAP: u16 = 0;

#[derive(Debug, Clone, PartialEq)]
pub struct Station {
    pub name: String,
    pub unit_id: u8,
    pub addressing: Addressing,
    pub max_retries: u32,
}

impl From<&ModbusStation> for Station {
    fn from(s: &ModbusStation) -> Self {
        Self { name: s.name.clone(), unit_id: s.unit_id, addressing: s.addressing, max_retries: s.max_retries }
    }
}

impl Station {
    /// Protocol address of configured register `register`, or `None` when the
    /// register does not exist under this station's addressing (0 when one-based).
    pub fn protocol_address(&self, register: u16) -> Option<u16> {
        match self.addressing {
            Addressing::OneBased => register.checked_sub(1),
            Addressing::ZeroBased => Some(register),
        }
    }
}

/// One tag inside a read: where its data starts relative to the read.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub id: TagId,
    pub offset: u16,
    pub data_type: DataType,
}

/// A read request against one station.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadJob {
    pub station: String,
    pub unit_id: u8,
    pub area: ModbusArea,
    pub address: u16,
    pub count: u16,
    pub slots: Vec<Slot>,
}

/// The driver's view of the project: stations and the reads to issue each cycle.
#[derive(Debug, Clone, Default)]
pub struct DriverPlan {
    pub stations: HashMap<String, Station>,
    pub jobs: Vec<ReadJob>,
    /// Tags the driver owns (polled or write-only), for marking quality.
    pub tags: Vec<TagId>,
    /// Tags whose address cannot be read (unknown station, register 0 when
    /// one-based). They stay bad.
    pub unreadable: Vec<(TagId, String)>,
}

impl DriverPlan {
    pub fn build(driver: &Driver, store: &TagStore) -> Self {
        let DriverConfig::ModbusTcp(cfg) = &driver.config;
        let stations: HashMap<String, Station> =
            cfg.stations.iter().map(|s| (s.name.clone(), Station::from(s))).collect();

        // plan_polls works on a project; hand it only this driver's tags.
        let tags: Vec<_> = store
            .ids()
            .map(|id| store.def(id))
            .filter(|t| t.address.as_ref().is_some_and(|a| a.driver() == driver.name))
            .cloned()
            .collect();
        let owned: Vec<TagId> = tags.iter().filter_map(|t| store.id(&t.name)).collect();
        let project = Project { tags, drivers: vec![driver.clone()], ..Project::default() };

        let mut plan = DriverPlan { stations, tags: owned, ..DriverPlan::default() };
        for job in plan_polls(&project, MAX_GAP) {
            let ids: Vec<TagId> = job.tags.iter().filter_map(|n| store.id(n)).collect();
            let Some(station) = plan.stations.get(&job.station) else {
                plan.unreadable.extend(ids.into_iter().map(|id| (id, format!("unknown station {:?}", job.station))));
                continue;
            };
            let Some(address) = station.protocol_address(job.start) else {
                plan.unreadable.extend(ids.into_iter().map(|id| (id, "register 0 does not exist with one-based addressing".to_string())));
                continue;
            };
            let slots = ids
                .into_iter()
                .filter_map(|id| {
                    let def = store.def(id);
                    let moviclone_model::Address::Modbus(a) = def.address.as_ref()?;
                    Some(Slot { id, offset: a.register - job.start, data_type: def.data_type })
                })
                .collect();
            plan.jobs.push(ReadJob {
                station: job.station.clone(),
                unit_id: station.unit_id,
                area: job.area,
                address,
                count: job.count,
                slots,
            });
        }
        plan
    }

    /// Tags read by the jobs of `station`.
    pub fn station_tags(&self, station: &str) -> impl Iterator<Item = TagId> + '_ {
        let station = station.to_string();
        self.jobs.iter().filter(move |j| j.station == station).flat_map(|j| j.slots.iter().map(|s| s.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moviclone_model::{Access, Address, ModbusAddress, ModbusTcp, ModbusWrite, Tag};

    pub(crate) fn tag(name: &str, ty: DataType, station: &str, area: ModbusArea, register: u16) -> Tag {
        Tag {
            name: name.into(),
            description: None,
            data_type: ty,
            access: Access::ReadWrite,
            address: Some(Address::Modbus(ModbusAddress {
                driver: "d".into(),
                station: station.into(),
                area,
                register,
                write: ModbusWrite::Single,
            })),
            historian: None,
            legacy_id: None,
        }
    }

    fn driver() -> Driver {
        Driver {
            name: "d".into(),
            config: DriverConfig::ModbusTcp(ModbusTcp {
                description: None,
                host: "127.0.0.1".into(),
                port: 502,
                poll_ms: 100,
                timeout_ms: 1000,
                keep_open: true,
                stations: vec![
                    ModbusStation { name: "A".into(), unit_id: 1, addressing: Addressing::OneBased, max_retries: 3 },
                    ModbusStation { name: "Z".into(), unit_id: 9, addressing: Addressing::ZeroBased, max_retries: 3 },
                ],
            }),
        }
    }

    #[test]
    fn one_based_registers_are_shifted() {
        let p = Project {
            tags: vec![
                tag("f1", DataType::F32, "A", ModbusArea::HoldingRegister, 247),
                tag("f2", DataType::F32, "A", ModbusArea::HoldingRegister, 249),
                tag("z", DataType::U16, "Z", ModbusArea::InputRegister, 10),
                tag("zero", DataType::U16, "A", ModbusArea::InputRegister, 0),
                tag("lost", DataType::U16, "nope", ModbusArea::InputRegister, 5),
            ],
            ..Project::default()
        };
        let store = TagStore::new(&p);
        let plan = DriverPlan::build(&driver(), &store);
        let hr = plan.jobs.iter().find(|j| j.area == ModbusArea::HoldingRegister).unwrap();
        assert_eq!((hr.unit_id, hr.address, hr.count), (1, 246, 4));
        assert_eq!(hr.slots.iter().map(|s| s.offset).collect::<Vec<_>>(), vec![0, 2]);
        let z = plan.jobs.iter().find(|j| j.station == "Z").unwrap();
        assert_eq!((z.unit_id, z.address), (9, 10));
        assert_eq!(plan.unreadable.len(), 2);
        assert_eq!(plan.tags.len(), 5);
    }
}
