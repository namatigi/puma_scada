//! Typed project model for Moviclone.
//!
//! A project is a folder of small TOML files (see [`io`]). Every file deserializes
//! straight into these types, so a malformed address, an unknown data type or a
//! plain-text password is rejected when the project loads, not when the plant runs.
//! Cross-references between files (tag names, stations, screens) are checked by the
//! `moviclone-check` crate.

use serde::{Deserialize, Serialize};
use std::fmt;

pub mod io;
pub use io::{load_project, save_project, LoadError, SourceMap};

pub mod alarm;
pub use alarm::{Alarm, AlarmCondition, AlarmSeverity};

/// A whole project, assembled from its files. Also the shape of the JSON
/// "bundle" that the runtime serves to the HMI and studio (`GET /api/bundle`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub meta: ProjectMeta,
    #[serde(default)]
    pub tags: Vec<Tag>,
    #[serde(default)]
    pub drivers: Vec<Driver>,
    #[serde(default)]
    pub data: DataConfig,
    #[serde(default)]
    pub alarms: Vec<Alarm>,
    #[serde(default)]
    pub screens: Vec<Screen>,
}

impl Project {
    pub fn tag(&self, name: &str) -> Option<&Tag> {
        self.tags.iter().find(|t| t.name == name)
    }
    pub fn screen(&self, name: &str) -> Option<&Screen> {
        self.screens.iter().find(|s| s.name == name)
    }
    pub fn driver(&self, name: &str) -> Option<&Driver> {
        self.drivers.iter().find(|d| d.name == name)
    }
}

// ---------------------------------------------------------------- project.toml

/// `project.toml`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMeta {
    pub name: String,
    /// Where the project runs, e.g. the depot name; shown in the HMI's top bar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_culture")]
    pub culture: String,
    /// Screen shown at start-up (a screen name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_screen: Option<String>,
    #[serde(default)]
    pub sampling: Sampling,
    /// Report layouts the project refers to. Not converted yet; listed so that
    /// report commands can be checked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reports: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
}

impl Default for ProjectMeta {
    fn default() -> Self {
        Self {
            name: "untitled".into(),
            site: None,
            description: None,
            culture: default_culture(),
            main_screen: None,
            sampling: Sampling::default(),
            reports: Vec::new(),
            origin: None,
        }
    }
}

fn default_culture() -> String {
    "en-us".into()
}

/// Subscription timing for clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sampling {
    pub fast_ms: u32,
    pub slow_ms: u32,
    pub publish_ms: u32,
}

impl Default for Sampling {
    fn default() -> Self {
        Self { fast_ms: 500, slow_ms: 5000, publish_ms: 250 }
    }
}

/// Where the project was imported from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub tool: String,
    pub version: String,
    pub project_id: String,
}

// ---------------------------------------------------------------- tags.toml

/// One process value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tag {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub data_type: DataType,
    #[serde(default, skip_serializing_if = "is_default")]
    pub access: Access,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<Address>,
    /// Historian that records this tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub historian: Option<String>,
    /// Identifier in the system the tag was imported from (Movicon NodeId GUID).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataType {
    Bool,
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
    String,
}

impl DataType {
    /// 16-bit registers the value occupies; `None` for single bits.
    /// Strings use 32 bytes, the Movicon Modbus default.
    pub fn registers(self) -> Option<u16> {
        match self {
            DataType::Bool => None,
            DataType::I8 | DataType::U8 | DataType::I16 | DataType::U16 => Some(1),
            DataType::I32 | DataType::U32 | DataType::F32 => Some(2),
            DataType::I64 | DataType::U64 | DataType::F64 => Some(4),
            DataType::String => Some(16),
        }
    }
}

/// Direction of the link to the field device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Access {
    /// Polled from the device; writes are refused.
    #[default]
    Read,
    /// Polled, and written back when changed.
    ReadWrite,
    /// Written only, never polled.
    Write,
}

impl Access {
    pub fn reads(self) -> bool {
        matches!(self, Access::Read | Access::ReadWrite)
    }
    pub fn writes(self) -> bool {
        matches!(self, Access::ReadWrite | Access::Write)
    }
}

/// Where a tag lives on a device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Address {
    Modbus(ModbusAddress),
}

impl Address {
    pub fn driver(&self) -> &str {
        match self {
            Address::Modbus(m) => &m.driver,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModbusAddress {
    /// Driver file name in `drivers/`, without extension.
    pub driver: String,
    pub station: String,
    pub area: ModbusArea,
    /// Register or bit number as configured, interpreted with the station's
    /// `addressing` (one-based or zero-based).
    pub register: u16,
    /// How writes are sent. Only meaningful for coils and holding registers.
    #[serde(default, skip_serializing_if = "is_default")]
    pub write: ModbusWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModbusArea {
    Coil,
    DiscreteInput,
    InputRegister,
    HoldingRegister,
}

impl ModbusArea {
    pub fn is_bit(self) -> bool {
        matches!(self, ModbusArea::Coil | ModbusArea::DiscreteInput)
    }
    pub fn is_writable(self) -> bool {
        matches!(self, ModbusArea::Coil | ModbusArea::HoldingRegister)
    }
    /// Largest single read the protocol allows, in bits or registers.
    pub fn max_read(self) -> u16 {
        if self.is_bit() {
            2000
        } else {
            125
        }
    }
}

impl fmt::Display for ModbusArea {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ModbusArea::Coil => "coil",
            ModbusArea::DiscreteInput => "discrete-input",
            ModbusArea::InputRegister => "input-register",
            ModbusArea::HoldingRegister => "holding-register",
        })
    }
}

/// Write function: single (FC5/FC6), multiple (FC15/FC16) or masked (FC22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModbusWrite {
    #[default]
    Multiple,
    Single,
    Mask,
}

// ---------------------------------------------------------------- drivers/*.toml

/// One driver instance, stored as `drivers/<name>.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Driver {
    /// File stem; tags refer to the driver by this name.
    pub name: String,
    #[serde(flatten)]
    pub config: DriverConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DriverConfig {
    ModbusTcp(ModbusTcp),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModbusTcp {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub host: String,
    #[serde(default = "default_modbus_port")]
    pub port: u16,
    #[serde(default = "default_poll_ms")]
    pub poll_ms: u32,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u32,
    #[serde(default = "default_true")]
    pub keep_open: bool,
    #[serde(rename = "station", default)]
    pub stations: Vec<ModbusStation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModbusStation {
    pub name: String,
    pub unit_id: u8,
    #[serde(default, skip_serializing_if = "is_default")]
    pub addressing: Addressing,
    #[serde(default = "default_retries")]
    pub max_retries: u32,
}

/// How configured register numbers map to protocol addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Addressing {
    /// Register 1 is protocol address 0.
    #[default]
    OneBased,
    /// Register 0 is protocol address 0.
    ZeroBased,
}

fn default_modbus_port() -> u16 {
    502
}
fn default_poll_ms() -> u32 {
    500
}
fn default_timeout_ms() -> u32 {
    5000
}
fn default_retries() -> u32 {
    3
}
fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------- data.toml

/// `data.toml`: databases, historians and data loggers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataConfig {
    #[serde(rename = "database", default, skip_serializing_if = "Vec::is_empty")]
    pub databases: Vec<Database>,
    #[serde(rename = "historian", default, skip_serializing_if = "Vec::is_empty")]
    pub historians: Vec<Historian>,
    #[serde(rename = "logger", default, skip_serializing_if = "Vec::is_empty")]
    pub loggers: Vec<DataLogger>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Database {
    pub name: String,
    pub provider: DbProvider,
    pub server: String,
    pub database: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Always a reference into the secret store, never the password itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<SecretRef>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub integrated_auth: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DbProvider {
    Mssql,
    Postgres,
    Sqlite,
}

/// A `secret://name` reference, resolved from the OS keychain or environment at
/// start-up. Anything else fails to deserialize, so a password typed into a
/// project file is rejected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SecretRef(String);

impl SecretRef {
    pub const PREFIX: &'static str = "secret://";

    pub fn new(name: &str) -> Result<Self, String> {
        Self::try_from(format!("{}{}", Self::PREFIX, name))
    }
    /// The name after `secret://`.
    pub fn name(&self) -> &str {
        &self.0[Self::PREFIX.len()..]
    }
}

impl TryFrom<String> for SecretRef {
    type Error = String;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        let name = s.strip_prefix(Self::PREFIX).ok_or_else(|| {
            "expected a secret reference like \"secret://main-sql\"; passwords must not be stored in project files".to_string()
        })?;
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
            return Err(format!("invalid secret name {name:?}: use letters, digits, '-', '_' or '.'"));
        }
        Ok(Self(s))
    }
}

impl From<SecretRef> for String {
    fn from(s: SecretRef) -> String {
        s.0
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Records tag values over time (trends).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Historian {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub database: String,
    /// Records older than this are deleted.
    pub max_age_s: u64,
}

/// Writes rows of selected tags into a table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataLogger {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub database: String,
    pub table: String,
    pub record: Recording,
    pub max_age_s: u64,
    #[serde(rename = "column", default)]
    pub columns: Vec<LoggerColumn>,
}

/// `record = "on-change"` or `record = { every-s = 60 }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Recording {
    /// A row whenever any column's tag changes.
    OnChange,
    /// A row every N seconds.
    EveryS(u32),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggerColumn {
    pub name: String,
    pub tag: String,
}

// ---------------------------------------------------------------- screens/*.toml

/// One operator screen, stored as `screens/<name>.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Screen {
    pub name: String,
    pub width: f64,
    pub height: f64,
    /// CSS colour or gradient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_id: Option<String>,
    #[serde(rename = "element", default)]
    pub elements: Vec<Element>,
}

/// A positioned object on a screen. Coordinates are in screen units, origin top-left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Element {
    pub id: String,
    #[serde(flatten)]
    pub kind: ElementKind,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Clockwise, in degrees, about the element's centre.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_click: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ElementKind {
    /// Static text.
    Label { text: String },
    /// Shows a tag value; operators can type a new value when `editable`.
    Display {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
        /// .NET-style numeric format, e.g. `#0.000`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
        #[serde(default, skip_serializing_if = "is_default")]
        editable: bool,
    },
    Button { text: String },
    /// A pipe drawn through `points`, relative to the element's x/y.
    Pipeline {
        points: Vec<[f64; 2]>,
        thickness: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stroke: Option<String>,
        /// Tag that animates the pipe: while it is flowing (see `flow_min`)
        /// dashes move along the pipe.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
        /// Colour of the moving dashes, usually the product's colour
        /// (Gasoline `#FF4D4D`, Jet A-1 `#F4F4F4`, ...). Default: the theme's flow colour.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flow_color: Option<String>,
        /// The pipe counts as flowing when the tag is above this value (for
        /// example 1 mTon/hr, so meter noise at standstill does not animate).
        /// Default: any non-zero / true value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flow_min: Option<f64>,
        /// Product moves against the order of `points` (positive tag values).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        flow_reverse: bool,
    },
    /// Graphic from a symbol library. `symbol` names a library entry; imported
    /// Movicon symbols keep their original name as `movicon:<SymbolName>` until
    /// they are mapped to a Moviclone symbol.
    Symbol {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        symbol: Option<String>,
        /// Tag that drives the symbol's state (open/closed, running, colour).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
    },
    /// Live and historical trend of one or more tags.
    Trend {
        #[serde(default)]
        tags: Vec<String>,
        /// Historian to read past values from.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        historian: Option<String>,
        /// Visible time window in seconds.
        #[serde(default = "default_trend_span")]
        span_s: u32,
    },
    /// Table of recorded rows from a historian or data logger.
    HistoryTable {
        source: String,
    },
    /// Active and recent alarms, optionally limited to one area.
    AlarmList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        area: Option<String>,
    },
}

fn default_trend_span() -> u32 {
    3600
}

/// What happens when an element is clicked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Command {
    OpenScreen {
        screen: String,
    },
    WriteTag {
        tag: String,
        value: String,
        #[serde(default, skip_serializing_if = "is_default")]
        mode: WriteMode,
    },
    /// Fill a report layout with tag values and export it.
    Report {
        report: String,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_dir: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WriteMode {
    #[default]
    Set,
    Increase,
    Decrease,
    Toggle,
    /// Write the value, then restore the previous value.
    Pulse,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}
