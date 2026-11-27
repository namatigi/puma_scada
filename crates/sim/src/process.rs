//! Physics of a petroleum loading manifold, one Coriolis flow meter per station.
//!
//! Units follow the Petrogas screens: mass flow in mTon/hr, density in kg/m³,
//! temperature in °C, volume rate in m³/hr (mass flow × 1000 / density),
//! totals in mTon and m³, pressure in bar.

use std::f64::consts::TAU;

/// Small deterministic PRNG (xorshift64*), so test runs are reproducible.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    /// Uniform in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_f64()
    }
    /// Roughly normal, mean 0, standard deviation ≈ `sd`.
    pub fn noise(&mut self, sd: f64) -> f64 {
        (self.next_f64() + self.next_f64() + self.next_f64() - 1.5) * 2.0 * sd
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Product {
    Gasoline,
    Jet,
    Gasoil,
    Hfo,
}

impl Product {
    // The OXOAFRICA meters, as the screens name them (FM_01 panel "HFO" …).
    const BY_METER: [Product; 4] = [Product::Hfo, Product::Gasoil, Product::Jet, Product::Gasoline];

    /// Product of meter number `n` (FM_01 → HFO, FM_02 → gasoil, FM_03 → JET A-1, FM_04 → gasoline).
    pub fn for_meter(n: u32) -> Product {
        Self::BY_METER[(n.max(1) as usize - 1) % Self::BY_METER.len()]
    }

    /// A product named in a tag or station name.
    pub fn from_name(name: &str) -> Option<Product> {
        let n = name.to_ascii_lowercase();
        [
            ("gasoline", Product::Gasoline),
            ("jet", Product::Jet),
            ("gasoil", Product::Gasoil),
            ("hfo", Product::Hfo),
        ]
        .into_iter()
        .find(|(k, _)| n.contains(k))
        .map(|(_, p)| p)
    }

    /// Density at 15 °C, kg/m³.
    pub fn density15(self) -> f64 {
        match self {
            Product::Gasoline => 720.0,
            Product::Jet => 800.0,
            Product::Gasoil => 840.0,
            Product::Hfo => 980.0,
        }
    }

    /// Typical loading rate, mTon/hr.
    fn nominal_flow(self) -> f64 {
        match self {
            Product::Gasoline => 118.0,
            Product::Jet => 104.0,
            Product::Gasoil => 110.0,
            Product::Hfo => 72.0,
        }
    }

    /// Product temperature, °C (heavy products are kept warm).
    fn base_temp(self) -> f64 {
        match self {
            Product::Gasoline => 24.0,
            Product::Jet => 26.0,
            Product::Gasoil => 28.0,
            Product::Hfo => 41.0,
        }
    }
}

/// What a tag measures, recognised from its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    MassFlow,
    Density,
    Temp,
    VolumeRate,
    MassTotal,
    VolTotal,
    MassInv,
    VolInv,
    Pressure,
    ExternalTemp,
    /// Totalizer start/stop coil.
    Start,
    /// Writing `true` resets the meter's totals.
    ResetTotal,
    /// Anything else: slow random walk.
    Other,
}

pub fn classify(tag: &str) -> Signal {
    let n = tag.to_ascii_lowercase();
    let has = |k: &str| n.contains(k);
    if has("massflow") || has("mass_flow") || has("mass_rate") {
        Signal::MassFlow
    } else if has("volume_rate") || has("vol_rate") || has("volflow") || has("vol_flow") {
        Signal::VolumeRate
    } else if has("density") {
        Signal::Density
    } else if has("reset") {
        Signal::ResetTotal
    } else if has("start") {
        Signal::Start
    } else if has("pressure") {
        Signal::Pressure
    } else if has("external_temp") || has("ext_temp") {
        Signal::ExternalTemp
    } else if has("temp") {
        Signal::Temp
    } else if has("mass_inv") {
        Signal::MassInv
    } else if has("vol_inv") || has("volume_inv") {
        Signal::VolInv
    } else if has("mass_total") || has("mass_tot") {
        Signal::MassTotal
    } else if n.ends_with("_vol") || has("vol_total") || has("volume_total") {
        Signal::VolTotal
    } else {
        Signal::Other
    }
}

/// One flow meter on the manifold.
#[derive(Debug, Clone)]
pub struct Meter {
    pub product: Product,
    rng: Rng,
    phase: f64,
    period_s: f64,
    pub flow: f64,
    pub running: bool,
    next_switch_s: f64,
    pub temp: f64,
    pub density: f64,
    pub pressure: f64,
    pub ext_temp: f64,
    pub mass_total: f64,
    pub vol_total: f64,
    pub mass_inv: f64,
    pub vol_inv: f64,
}

/// Time constant of the flow following its set point (valves, pumps), s.
const FLOW_TAU_S: f64 = 4.0;

impl Meter {
    pub fn new(product: Product, seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let phase = rng.range(0.0, TAU);
        let period_s = rng.range(240.0, 600.0);
        let mass_total = rng.range(20.0, 400.0);
        let mass_inv = rng.range(8_000.0, 60_000.0);
        let next_switch_s = rng.range(120.0, 480.0);
        let rho = product.density15();
        let mut m = Self {
            product,
            rng,
            phase,
            period_s,
            flow: product.nominal_flow() * 0.8,
            running: true,
            next_switch_s,
            temp: product.base_temp(),
            density: rho,
            pressure: 4.0,
            ext_temp: product.base_temp(),
            mass_total,
            vol_total: mass_total * 1000.0 / rho,
            mass_inv,
            vol_inv: mass_inv * 1000.0 / rho,
        };
        m.step(0.0, 0.0, false);
        m
    }

    pub fn volume_rate(&self) -> f64 {
        if self.density > 0.0 {
            self.flow * 1000.0 / self.density
        } else {
            0.0
        }
    }

    /// Advances to time `t` (s since start) by `dt` seconds. Totals integrate
    /// only while `totalizing` (the START coil); inventories always do.
    pub fn step(&mut self, t: f64, dt: f64, totalizing: bool) {
        if t >= self.next_switch_s {
            self.running = !self.running;
            // Loading runs last minutes; truck changes and stops are short.
            self.next_switch_s = t + if self.running { self.rng.range(180.0, 600.0) } else { self.rng.range(40.0, 120.0) };
        }
        let wave = (TAU * t / self.period_s + self.phase).sin();
        let target = if self.running {
            (self.product.nominal_flow() * (0.82 + 0.15 * wave) + self.rng.noise(1.2)).clamp(0.0, 150.0)
        } else {
            0.0
        };
        let k = if dt > 0.0 { 1.0 - (-dt / FLOW_TAU_S).exp() } else { 1.0 };
        self.flow += (target - self.flow) * k;
        if !self.running && self.flow < 0.05 {
            self.flow = 0.0;
        }
        self.flow = self.flow.clamp(0.0, 150.0);

        let slow = (TAU * t / 1800.0 + self.phase).sin();
        self.temp = (self.product.base_temp() + 2.5 * slow + self.rng.noise(0.03)).clamp(20.0, 45.0);
        // Thermal expansion of petroleum products, roughly 0.095 %/K.
        self.density = self.product.density15() * (1.0 - 0.00095 * (self.temp - self.product.base_temp())) + self.rng.noise(0.05);
        self.pressure = (2.2 + 5.0 * self.flow / 150.0 + 0.2 * wave + self.rng.noise(0.02)).clamp(2.0, 8.0);
        self.ext_temp = (self.temp + 0.6 + self.rng.noise(0.05)).clamp(15.0, 50.0);

        let mass = self.flow * dt / 3600.0;
        let vol = self.volume_rate() * dt / 3600.0;
        if totalizing {
            self.mass_total += mass;
            self.vol_total += vol;
        }
        self.mass_inv += mass;
        self.vol_inv += vol;
    }

    pub fn reset_totals(&mut self) {
        self.mass_total = 0.0;
        self.vol_total = 0.0;
    }

    pub fn value(&self, s: Signal) -> Option<f64> {
        Some(match s {
            Signal::MassFlow => self.flow,
            Signal::Density => self.density,
            Signal::Temp => self.temp,
            Signal::VolumeRate => self.volume_rate(),
            Signal::MassTotal => self.mass_total,
            Signal::VolTotal => self.vol_total,
            Signal::MassInv => self.mass_inv,
            Signal::VolInv => self.vol_inv,
            Signal::Pressure => self.pressure,
            Signal::ExternalTemp => self.ext_temp,
            Signal::Start | Signal::ResetTotal | Signal::Other => return None,
        })
    }

    pub fn rng(&mut self) -> &mut Rng {
        &mut self.rng
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_petrogas_tags() {
        for (name, s) in [
            ("FM_01_MassFlow", Signal::MassFlow),
            ("FM_01_Density", Signal::Density),
            ("FM_01_Temp", Signal::Temp),
            ("FM_01_Volume_Rate", Signal::VolumeRate),
            ("FM_01_Mass_Total", Signal::MassTotal),
            ("FM_01_Vol", Signal::VolTotal),
            ("FM_01_Mass_Inv", Signal::MassInv),
            ("FM_01_Vol_Inv", Signal::VolInv),
            ("FM_01_External_Pressure", Signal::Pressure),
            ("FM_01_External_Temp", Signal::ExternalTemp),
            ("FM_02_External_Temperature", Signal::ExternalTemp),
            ("FM_01_START", Signal::Start),
            ("FM_01_Reset_Total", Signal::ResetTotal),
            ("Flow_Direction", Signal::Other),
        ] {
            assert_eq!(classify(name), s, "{name}");
        }
    }

    #[test]
    fn products_by_meter_number() {
        assert_eq!(Product::for_meter(1), Product::Hfo);
        assert_eq!(Product::for_meter(2), Product::Gasoil);
        assert_eq!(Product::for_meter(3), Product::Jet);
        assert_eq!(Product::for_meter(4), Product::Gasoline);
        assert_eq!(Product::from_name("HFO_Meter"), Some(Product::Hfo));
    }

    #[test]
    fn an_hour_of_loading_stays_plausible() {
        for (i, p) in Product::BY_METER.into_iter().enumerate() {
            let mut m = Meter::new(p, i as u64 + 1);
            let (t0, inv0) = (m.mass_total, m.mass_inv);
            let mut stopped = false;
            let mut t = 0.0;
            while t < 3600.0 {
                t += 0.5;
                m.step(t, 0.5, true);
                assert!((0.0..=150.0).contains(&m.flow));
                assert!((20.0..=45.0).contains(&m.temp));
                assert!((2.0..=8.0).contains(&m.pressure));
                assert!((m.density - p.density15()).abs() < 40.0, "{p:?} density {}", m.density);
                stopped |= m.flow == 0.0;
            }
            assert!(stopped, "{p:?} never stopped within an hour");
            let moved = m.mass_total - t0;
            assert!(moved > 10.0 && moved < 150.0, "{p:?} moved {moved} t");
            assert!((m.mass_inv - inv0 - moved).abs() < 1e-6);
        }
    }

    #[test]
    fn totals_hold_while_not_totalizing_and_reset() {
        let mut m = Meter::new(Product::Jet, 7);
        let before = m.mass_total;
        let inv = m.mass_inv;
        m.step(1.0, 1.0, false);
        assert_eq!(m.mass_total, before);
        assert!(m.mass_inv > inv);
        m.reset_totals();
        assert_eq!((m.mass_total, m.vol_total), (0.0, 0.0));
    }
}
