//! The simulated devices' memory (coils, registers per unit id), the tags bound
//! to it, and the Modbus request handling on top.

use crate::process::{classify, Meter, Product, Signal};
use moviclone_driver_modbus::codec::{decode_bit, decode_registers, encode_bit, encode_registers, register_count};
use moviclone_model::{Address, Addressing, DataType, DriverConfig, ModbusArea, Project};
use moviclone_tagdb::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use tokio_modbus::{Exception, Request, Response};

/// Memory of one unit id. Addresses are protocol addresses (zero-based).
#[derive(Debug, Default, Clone)]
pub struct Unit {
    coils: HashMap<u16, bool>,
    discrete: HashMap<u16, bool>,
    input: HashMap<u16, u16>,
    holding: HashMap<u16, u16>,
}

impl Unit {
    fn bits(&mut self, area: ModbusArea) -> &mut HashMap<u16, bool> {
        if area == ModbusArea::Coil {
            &mut self.coils
        } else {
            &mut self.discrete
        }
    }
    fn words(&mut self, area: ModbusArea) -> &mut HashMap<u16, u16> {
        if area == ModbusArea::HoldingRegister {
            &mut self.holding
        } else {
            &mut self.input
        }
    }
}

/// A tag as the simulator sees it.
#[derive(Debug, Clone)]
struct Binding {
    tag: String,
    unit: u8,
    area: ModbusArea,
    addr: u16,
    ty: DataType,
    signal: Signal,
    meter: usize,
    /// State of the random walk for tags without a known meaning.
    walk: f64,
}

impl Binding {
    fn words(&self) -> impl Iterator<Item = u16> {
        let n = if self.area.is_bit() { 1 } else { register_count(self.ty) as u16 };
        self.addr..self.addr.saturating_add(n)
    }
}

pub struct SimState {
    units: HashMap<u8, Unit>,
    bindings: Vec<Binding>,
    by_name: HashMap<String, usize>,
    meters: Vec<Meter>,
    /// Addresses a client wrote: the simulation leaves them alone from then on.
    pinned: HashSet<(u8, ModbusArea, u16)>,
    t: f64,
}

/// Digits at the end of `FM_01_…`-style or `Flow_Meter_1`-style names.
fn meter_number(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    let rest = lower.strip_prefix("fm_").or_else(|| lower.strip_prefix("fm"))?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn trailing_number(name: &str) -> Option<u32> {
    let digits: String = name.chars().rev().take_while(char::is_ascii_digit).collect();
    digits.chars().rev().collect::<String>().parse().ok()
}

impl SimState {
    /// Builds the devices of every modbus-tcp driver in `project`. Stations of
    /// different drivers that share a unit id share one device image.
    pub fn new(project: &Project) -> Self {
        // (driver, station) -> (unit id, addressing)
        let mut stations = HashMap::new();
        let mut owners: BTreeMap<u8, String> = BTreeMap::new();
        for d in &project.drivers {
            let DriverConfig::ModbusTcp(cfg) = &d.config;
            for s in &cfg.stations {
                stations.insert((d.name.clone(), s.name.clone()), (s.unit_id, s.addressing));
                let owner = format!("{}/{}", d.name, s.name);
                if let Some(prev) = owners.get(&s.unit_id) {
                    tracing::warn!(unit_id = s.unit_id, "simulator: {owner} shares a unit id with {prev}");
                } else {
                    owners.insert(s.unit_id, owner);
                }
            }
        }

        // One meter per unit id; the product comes from the meter number in the
        // tag names (FM_01 → gasoline …), a product word, or the station order.
        let mut meter_of_unit: BTreeMap<u8, usize> = BTreeMap::new();
        let mut products: Vec<Option<Product>> = Vec::new();
        let mut bindings = Vec::new();
        for t in &project.tags {
            let Some(Address::Modbus(a)) = &t.address else { continue };
            let Some(&(unit, addressing)) = stations.get(&(a.driver.clone(), a.station.clone())) else {
                continue;
            };
            let addr = match addressing {
                Addressing::OneBased => match a.register.checked_sub(1) {
                    Some(r) => r,
                    None => continue,
                },
                Addressing::ZeroBased => a.register,
            };
            let meter = *meter_of_unit.entry(unit).or_insert_with(|| {
                products.push(None);
                products.len() - 1
            });
            let guess = meter_number(&t.name)
                .map(Product::for_meter)
                .or_else(|| Product::from_name(&t.name))
                .or_else(|| Product::from_name(&a.station))
                .or_else(|| trailing_number(&a.station).map(Product::for_meter));
            if products[meter].is_none() {
                products[meter] = guess;
            }
            bindings.push(Binding {
                tag: t.name.clone(),
                unit,
                area: a.area,
                addr,
                ty: t.data_type,
                signal: classify(&t.name),
                meter,
                walk: 50.0,
            });
        }
        let meters = products
            .iter()
            .enumerate()
            .map(|(i, p)| Meter::new(p.unwrap_or_else(|| Product::for_meter(i as u32 + 1)), i as u64 + 1))
            .collect();
        let mut units: HashMap<u8, Unit> = owners.keys().map(|&u| (u, Unit::default())).collect();
        for b in &bindings {
            let unit = units.entry(b.unit).or_default();
            if b.area.is_bit() {
                // Totalizers run from start-up; reset coils rest at false.
                unit.bits(b.area).insert(b.addr, b.signal == Signal::Start);
            }
        }
        let by_name = bindings.iter().enumerate().map(|(i, b)| (b.tag.clone(), i)).collect();
        let mut state = Self { units, bindings, by_name, meters, pinned: HashSet::new(), t: 0.0 };
        state.refresh();
        state
    }

    pub fn unit_ids(&self) -> Vec<u8> {
        let mut ids: Vec<u8> = self.units.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Advances the simulation by `dt` seconds and rewrites the device image.
    pub fn tick(&mut self, dt: f64) {
        self.t += dt;
        let t = self.t;
        for (i, meter) in self.meters.iter_mut().enumerate() {
            // Totalizing follows the meter's START coil when it has one.
            let start = self
                .bindings
                .iter()
                .find(|b| b.meter == i && b.signal == Signal::Start && b.area.is_bit())
                .map(|b| self.units.get(&b.unit).and_then(|u| u.coils.get(&b.addr).or(u.discrete.get(&b.addr))).copied());
            let totalizing = start.map_or(true, |v| v.unwrap_or(true));
            meter.step(t, dt, totalizing);
        }
        for b in &mut self.bindings {
            if b.signal == Signal::Other && !b.area.is_bit() {
                let rng = self.meters[b.meter].rng();
                b.walk = (b.walk + rng.noise(0.15 * dt.sqrt())).clamp(0.0, 100.0);
            }
        }
        self.refresh();
    }

    /// Writes the current process values into the registers of every tag no
    /// client has overwritten.
    fn refresh(&mut self) {
        for b in &self.bindings {
            if b.area.is_bit() || b.words().any(|w| self.pinned.contains(&(b.unit, b.area, w))) {
                continue;
            }
            let value = match self.meters[b.meter].value(b.signal) {
                Some(v) => v,
                None if b.signal == Signal::Other => b.walk,
                None => continue,
            };
            let value = match b.ty {
                DataType::F32 | DataType::F64 => Value::Float(value),
                DataType::Bool => Value::Bool(value >= 50.0),
                DataType::String => Value::Text(format!("{value:.2}")),
                _ => Value::Float(value.round()),
            };
            let Ok(words) = encode_registers(b.ty, &value) else { continue };
            let unit = self.units.entry(b.unit).or_default();
            let regs = unit.words(b.area);
            for (w, addr) in words.into_iter().zip(b.addr..) {
                regs.insert(addr, w);
            }
        }
    }

    /// Engineering value of a tag as a client would decode it now.
    pub fn read_tag(&self, name: &str) -> Option<Value> {
        let b = &self.bindings[*self.by_name.get(name)?];
        let unit = self.units.get(&b.unit)?;
        if b.area.is_bit() {
            let map = if b.area == ModbusArea::Coil { &unit.coils } else { &unit.discrete };
            return Some(decode_bit(b.ty, map.get(&b.addr).copied().unwrap_or(false)));
        }
        let map = if b.area == ModbusArea::HoldingRegister { &unit.holding } else { &unit.input };
        let words: Vec<u16> = b.words().map(|a| map.get(&a).copied().unwrap_or(0)).collect();
        decode_registers(b.ty, &words).ok()
    }

    /// Forces a tag to `value` (pinned like a client write, side effects included).
    pub fn write_tag(&mut self, name: &str, value: &Value) -> Result<(), String> {
        let b = self.bindings.get(*self.by_name.get(name).ok_or_else(|| format!("no simulated tag {name:?}"))?).cloned();
        let b = b.ok_or_else(|| format!("no simulated tag {name:?}"))?;
        if b.area.is_bit() {
            let bit = encode_bit(b.ty, value).map_err(|e| e.to_string())?;
            self.set_bits(b.unit, b.area, b.addr, &[bit]);
        } else {
            let words = encode_registers(b.ty, value).map_err(|e| e.to_string())?;
            self.set_words(b.unit, b.area, b.addr, &words);
        }
        Ok(())
    }

    fn set_bits(&mut self, unit: u8, area: ModbusArea, addr: u16, bits: &[bool]) {
        let image = self.units.entry(unit).or_default().bits(area);
        for (a, &bit) in (addr..).zip(bits) {
            image.insert(a, bit);
            self.pinned.insert((unit, area, a));
        }
        // Side effect: a reset coil set to true zeroes its meter's totals.
        let resets: Vec<usize> = self
            .bindings
            .iter()
            .filter(|b| b.unit == unit && b.area == area && b.signal == Signal::ResetTotal)
            .filter(|b| b.addr >= addr && ((b.addr - addr) as usize) < bits.len() && bits[(b.addr - addr) as usize])
            .map(|b| b.meter)
            .collect();
        for m in resets {
            self.meters[m].reset_totals();
            tracing::info!(unit, "simulator: totals reset");
        }
        self.refresh();
    }

    fn set_words(&mut self, unit: u8, area: ModbusArea, addr: u16, words: &[u16]) {
        let image = self.units.entry(unit).or_default().words(area);
        for (a, &w) in (addr..).zip(words) {
            image.insert(a, w);
            self.pinned.insert((unit, area, a));
        }
    }

    /// Answers one Modbus request for `unit`.
    pub fn handle(&mut self, unit: u8, req: Request<'_>) -> Result<Response, Exception> {
        if !self.units.contains_key(&unit) {
            return Err(Exception::GatewayTargetDevice);
        }
        let check = |addr: u16, count: u16, max: u16| -> Result<(), Exception> {
            if count == 0 || count > max {
                return Err(Exception::IllegalDataValue);
            }
            if u32::from(addr) + u32::from(count) > 0x1_0000 {
                return Err(Exception::IllegalDataAddress);
            }
            Ok(())
        };
        let read_bits = |map: &HashMap<u16, bool>, addr: u16, n: u16| -> Vec<bool> {
            (addr..addr.saturating_add(n)).map(|a| map.get(&a).copied().unwrap_or(false)).take(n as usize).collect()
        };
        let read_words = |map: &HashMap<u16, u16>, addr: u16, n: u16| -> Vec<u16> {
            (0..n).map(|i| map.get(&addr.wrapping_add(i)).copied().unwrap_or(0)).collect()
        };
        match req {
            Request::ReadCoils(addr, n) => {
                check(addr, n, 2000)?;
                Ok(Response::ReadCoils(read_bits(&self.units[&unit].coils, addr, n)))
            }
            Request::ReadDiscreteInputs(addr, n) => {
                check(addr, n, 2000)?;
                Ok(Response::ReadDiscreteInputs(read_bits(&self.units[&unit].discrete, addr, n)))
            }
            Request::ReadHoldingRegisters(addr, n) => {
                check(addr, n, 125)?;
                Ok(Response::ReadHoldingRegisters(read_words(&self.units[&unit].holding, addr, n)))
            }
            Request::ReadInputRegisters(addr, n) => {
                check(addr, n, 125)?;
                Ok(Response::ReadInputRegisters(read_words(&self.units[&unit].input, addr, n)))
            }
            Request::WriteSingleCoil(addr, bit) => {
                self.set_bits(unit, ModbusArea::Coil, addr, &[bit]);
                Ok(Response::WriteSingleCoil(addr, bit))
            }
            Request::WriteMultipleCoils(addr, bits) => {
                check(addr, bits.len() as u16, 1968)?;
                self.set_bits(unit, ModbusArea::Coil, addr, &bits);
                Ok(Response::WriteMultipleCoils(addr, bits.len() as u16))
            }
            Request::WriteSingleRegister(addr, w) => {
                self.set_words(unit, ModbusArea::HoldingRegister, addr, &[w]);
                Ok(Response::WriteSingleRegister(addr, w))
            }
            Request::WriteMultipleRegisters(addr, words) => {
                check(addr, words.len() as u16, 123)?;
                self.set_words(unit, ModbusArea::HoldingRegister, addr, &words);
                Ok(Response::WriteMultipleRegisters(addr, words.len() as u16))
            }
            Request::MaskWriteRegister(addr, and, or) => {
                let cur = self.units[&unit].holding.get(&addr).copied().unwrap_or(0);
                self.set_words(unit, ModbusArea::HoldingRegister, addr, &[(cur & and) | (or & !and)]);
                Ok(Response::MaskWriteRegister(addr, and, or))
            }
            Request::ReadWriteMultipleRegisters(raddr, n, waddr, words) => {
                check(raddr, n, 125)?;
                check(waddr, words.len() as u16, 121)?;
                self.set_words(unit, ModbusArea::HoldingRegister, waddr, &words);
                let img = &self.units[&unit].holding;
                Ok(Response::ReadWriteMultipleRegisters(read_words(img, raddr, n)))
            }
            Request::Custom(..) | Request::Disconnect => Err(Exception::IllegalFunction),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moviclone_model::{Access, Driver, ModbusAddress, ModbusStation, ModbusTcp, ModbusWrite, Tag};

    fn tag(name: &str, ty: DataType, station: &str, area: ModbusArea, register: u16) -> Tag {
        Tag {
            name: name.into(),
            description: None,
            data_type: ty,
            access: Access::ReadWrite,
            address: Some(Address::Modbus(ModbusAddress {
                driver: "fm".into(),
                station: station.into(),
                area,
                register,
                write: ModbusWrite::Single,
            })),
            historian: None,
            legacy_id: None,
        }
    }

    fn project() -> Project {
        let station = |name: &str, unit_id| ModbusStation {
            name: name.into(),
            unit_id,
            addressing: Addressing::OneBased,
            max_retries: 3,
        };
        Project {
            drivers: vec![Driver {
                name: "fm".into(),
                config: DriverConfig::ModbusTcp(ModbusTcp {
                    description: None,
                    host: "x".into(),
                    port: 502,
                    poll_ms: 100,
                    timeout_ms: 500,
                    keep_open: true,
                    stations: vec![station("Flow_Meter_1", 1), station("Flow_Meter_4", 4)],
                }),
            }],
            tags: vec![
                tag("FM_01_MassFlow", DataType::F32, "Flow_Meter_1", ModbusArea::HoldingRegister, 247),
                tag("FM_01_Density", DataType::F32, "Flow_Meter_1", ModbusArea::HoldingRegister, 249),
                tag("FM_01_Mass_Total", DataType::F32, "Flow_Meter_1", ModbusArea::HoldingRegister, 259),
                tag("FM_01_START", DataType::Bool, "Flow_Meter_1", ModbusArea::Coil, 2),
                tag("FM_01_Reset_Total", DataType::Bool, "Flow_Meter_1", ModbusArea::Coil, 3),
                tag("FM_04_Density", DataType::F32, "Flow_Meter_4", ModbusArea::HoldingRegister, 249),
                tag("Counter", DataType::U16, "Flow_Meter_4", ModbusArea::InputRegister, 10),
            ],
            ..Project::default()
        }
    }

    fn f(v: Option<Value>) -> f64 {
        v.and_then(|v| v.as_f64()).expect("numeric value")
    }

    #[test]
    fn densities_follow_the_product_of_each_meter() {
        let s = SimState::new(&project());
        assert!((f(s.read_tag("FM_01_Density")) - 720.0).abs() < 15.0);
        assert!((f(s.read_tag("FM_04_Density")) - 980.0).abs() < 25.0);
        assert_eq!(s.unit_ids(), vec![1, 4]);
    }

    #[test]
    fn registers_are_one_based_and_big_endian() {
        let mut s = SimState::new(&project());
        s.write_tag("FM_01_MassFlow", &Value::Float(123.5)).unwrap();
        let Ok(Response::ReadHoldingRegisters(w)) = s.handle(1, Request::ReadHoldingRegisters(246, 2)) else {
            panic!("read failed")
        };
        assert_eq!(w, vec![0x42F7, 0x0000]);
    }

    #[test]
    fn written_registers_persist_across_ticks() {
        let mut s = SimState::new(&project());
        s.handle(1, Request::WriteMultipleRegisters(248, vec![0x4448, 0x0000].into())).unwrap();
        for _ in 0..10 {
            s.tick(0.5);
        }
        assert_eq!(f(s.read_tag("FM_01_Density")), 800.0);
    }

    #[test]
    fn reset_coil_zeroes_totals_and_start_stops_them() {
        let mut s = SimState::new(&project());
        assert_eq!(s.read_tag("FM_01_START"), Some(Value::Bool(true)));
        s.handle(1, Request::WriteSingleCoil(1, false)).unwrap(); // START (register 2) off: totals frozen
        let before = f(s.read_tag("FM_01_Mass_Total"));
        s.tick(10.0);
        assert_eq!(f(s.read_tag("FM_01_Mass_Total")), before);
        s.handle(1, Request::WriteSingleCoil(2, true)).unwrap(); // Reset_Total (register 3)
        assert_eq!(f(s.read_tag("FM_01_Mass_Total")), 0.0);
        assert_eq!(s.read_tag("FM_01_Reset_Total"), Some(Value::Bool(true)));
        s.handle(1, Request::WriteSingleCoil(1, true)).unwrap();
        s.tick(60.0);
        assert!(f(s.read_tag("FM_01_Mass_Total")) > 0.0);
    }

    #[test]
    fn unknown_tags_random_walk_within_bounds() {
        let mut s = SimState::new(&project());
        for _ in 0..1000 {
            s.tick(1.0);
            let v = f(s.read_tag("Counter"));
            assert!((0.0..=100.0).contains(&v));
        }
    }

    #[test]
    fn bad_requests_get_exceptions() {
        let mut s = SimState::new(&project());
        assert_eq!(s.handle(9, Request::ReadCoils(0, 1)), Err(Exception::GatewayTargetDevice));
        assert_eq!(s.handle(1, Request::ReadHoldingRegisters(0, 126)), Err(Exception::IllegalDataValue));
        assert_eq!(s.handle(1, Request::ReadHoldingRegisters(65535, 2)), Err(Exception::IllegalDataAddress));
        assert_eq!(s.handle(1, Request::ReadCoils(1, 3)), Ok(Response::ReadCoils(vec![true, false, false])));
    }
}
