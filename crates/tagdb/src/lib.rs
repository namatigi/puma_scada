//! In-memory tag store: the current value, quality and timestamp of every tag,
//! a change bus, and routing of writes to the driver that owns each tag.
//!
//! Drivers call [`TagStore::update`] with fresh readings and register a write
//! channel with [`TagStore::register_writer`]. Everything else (gateway,
//! historian, alarms) reads with [`TagStore::get`] and listens on
//! [`TagStore::subscribe`].
//!
//! Tags are addressed by a dense [`TagId`] (index into the project's tag list),
//! so lookups on the hot path are array indexing, not hashing.

use moviclone_model::{Access, DataType, Project, Tag, WriteMode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc, oneshot};

pub type TagId = u32;

/// A tag value. Serializes as plain JSON (`null`, `true`, `12`, `1.5`, `"text"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(untagged)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl Value {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            Value::Text(s) => s.trim().parse().ok(),
            Value::Null => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            Value::Text(s) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "on" | "yes" => Some(true),
                "false" | "off" | "no" => Some(false),
                other => other.parse::<f64>().ok().map(|f| f != 0.0),
            },
            v => v.as_f64().map(|f| f != 0.0),
        }
    }

    /// Converts to the representation of `ty`, rejecting out-of-range numbers.
    pub fn coerce(&self, ty: DataType) -> Result<Value, String> {
        let num = || self.as_f64().ok_or_else(|| format!("{self} is not a number"));
        let int = |lo: f64, hi: f64| -> Result<Value, String> {
            let f = num()?;
            if f.fract() != 0.0 || f < lo || f > hi {
                return Err(format!("{f} does not fit {ty:?}"));
            }
            Ok(Value::Int(f as i64))
        };
        match ty {
            DataType::Bool => self.as_bool().map(Value::Bool).ok_or_else(|| format!("{self} is not a boolean")),
            DataType::I8 => int(i8::MIN as f64, i8::MAX as f64),
            DataType::U8 => int(0.0, u8::MAX as f64),
            DataType::I16 => int(i16::MIN as f64, i16::MAX as f64),
            DataType::U16 => int(0.0, u16::MAX as f64),
            DataType::I32 => int(i32::MIN as f64, i32::MAX as f64),
            DataType::U32 => int(0.0, u32::MAX as f64),
            DataType::I64 => int(i64::MIN as f64, i64::MAX as f64),
            DataType::U64 => int(0.0, u64::MAX as f64),
            DataType::F32 => num().map(|f| Value::Float(f as f32 as f64)),
            DataType::F64 => num().map(Value::Float),
            DataType::String => Ok(Value::Text(match self {
                Value::Text(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            })),
        }
    }

    fn default_for(ty: DataType) -> Value {
        match ty {
            DataType::Bool => Value::Bool(false),
            DataType::F32 | DataType::F64 => Value::Float(0.0),
            DataType::String => Value::Text(String::new()),
            _ => Value::Int(0),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => f.write_str("null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => write!(f, "{x}"),
            Value::Text(s) => write!(f, "{s:?}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    Good,
    /// Value may be stale or approximate (e.g. last known value after a retry).
    Uncertain,
    /// No valid value: device not answering, not yet read, decode error.
    Bad,
}

/// Value + quality + time of last update (milliseconds since the Unix epoch).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    #[serde(rename = "v")]
    pub value: Value,
    #[serde(rename = "q")]
    pub quality: Quality,
    pub ts: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub id: TagId,
    pub sample: Sample,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    UnknownTag(String),
    ReadOnly(String),
    BadValue(String),
    /// No driver has registered for the tag's driver name.
    NoDriver(String),
    /// The device rejected or failed the write.
    Device(String),
    Timeout,
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::UnknownTag(t) => write!(f, "tag {t:?} does not exist"),
            WriteError::ReadOnly(t) => write!(f, "tag {t:?} is read-only"),
            WriteError::BadValue(m) => write!(f, "invalid value: {m}"),
            WriteError::NoDriver(d) => write!(f, "driver {d:?} is not running"),
            WriteError::Device(m) => write!(f, "device error: {m}"),
            WriteError::Timeout => f.write_str("the device did not confirm the write in time"),
        }
    }
}

impl std::error::Error for WriteError {}

/// Sent to a driver's write channel. The value is already coerced to the tag's
/// data type. The driver replies once the device confirms, and should then
/// call [`TagStore::update`] with the new value.
#[derive(Debug)]
pub struct WriteRequest {
    pub id: TagId,
    pub value: Value,
    pub reply: oneshot::Sender<Result<(), WriteError>>,
}

pub const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a `pulse` write holds the value before returning to the resting value.
pub const PULSE: Duration = Duration::from_millis(500);

pub struct TagStore {
    defs: Vec<Tag>,
    names: HashMap<String, TagId>,
    cells: Vec<RwLock<Sample>>,
    changes: broadcast::Sender<Change>,
    writers: RwLock<HashMap<String, mpsc::Sender<WriteRequest>>>,
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl TagStore {
    /// Tags with a device address start as `null`/bad until first read; memory
    /// tags (no address) start at their type's zero value with good quality.
    pub fn new(project: &Project) -> Arc<Self> {
        let defs = project.tags.clone();
        let names = defs.iter().enumerate().map(|(i, t)| (t.name.clone(), i as TagId)).collect();
        let ts = now_ms();
        let cells = defs
            .iter()
            .map(|t| {
                RwLock::new(match t.address {
                    Some(_) => Sample { value: Value::Null, quality: Quality::Bad, ts },
                    None => Sample { value: Value::default_for(t.data_type), quality: Quality::Good, ts },
                })
            })
            .collect();
        let (changes, _) = broadcast::channel(8192);
        Arc::new(Self { defs, names, cells, changes, writers: RwLock::new(HashMap::new()) })
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }
    pub fn id(&self, name: &str) -> Option<TagId> {
        self.names.get(name).copied()
    }
    pub fn name(&self, id: TagId) -> &str {
        &self.defs[id as usize].name
    }
    pub fn def(&self, id: TagId) -> &Tag {
        &self.defs[id as usize]
    }
    pub fn ids(&self) -> impl Iterator<Item = TagId> {
        0..self.defs.len() as TagId
    }

    pub fn get(&self, id: TagId) -> Sample {
        self.cells[id as usize].read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn get_by_name(&self, name: &str) -> Option<Sample> {
        self.id(name).map(|id| self.get(id))
    }

    /// Records a reading. The value is coerced to the tag's type (a value that
    /// does not fit is stored as `null` with bad quality). Publishes a
    /// [`Change`] when value or quality differ from the stored sample, and
    /// returns whether it did.
    pub fn update(&self, id: TagId, value: Value, quality: Quality) -> bool {
        let (value, quality) = match value {
            Value::Null => (Value::Null, quality),
            v => match v.coerce(self.defs[id as usize].data_type) {
                Ok(v) => (v, quality),
                Err(_) => (Value::Null, Quality::Bad),
            },
        };
        let sample = {
            let mut cell = self.cells[id as usize].write().unwrap_or_else(|e| e.into_inner());
            let changed = cell.value != value || cell.quality != quality;
            cell.ts = now_ms();
            if !changed {
                return false;
            }
            cell.value = value;
            cell.quality = quality;
            cell.clone()
        };
        let _ = self.changes.send(Change { id, sample });
        true
    }

    /// Changes only the quality, keeping the last value (e.g. on a timeout).
    pub fn set_quality(&self, id: TagId, quality: Quality) -> bool {
        let value = self.get(id).value;
        self.update(id, value, quality)
    }

    /// A new receiver for all changes from now on. Slow receivers that fall more
    /// than 8192 changes behind get `RecvError::Lagged` and should re-read
    /// current values with [`TagStore::get`].
    pub fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    /// Routes writes for tags whose address names `driver` to `tx`.
    pub fn register_writer(&self, driver: &str, tx: mpsc::Sender<WriteRequest>) {
        self.writers.write().unwrap_or_else(|e| e.into_inner()).insert(driver.to_string(), tx);
    }

    /// Writes a value: to memory for tags without an address, otherwise to the
    /// owning driver, waiting for the device to confirm.
    pub async fn write(&self, id: TagId, value: Value) -> Result<(), WriteError> {
        let def = self.defs.get(id as usize).ok_or_else(|| WriteError::UnknownTag(id.to_string()))?;
        let value = value.coerce(def.data_type).map_err(WriteError::BadValue)?;
        let Some(address) = &def.address else {
            self.update(id, value, Quality::Good);
            return Ok(());
        };
        if !matches!(def.access, Access::ReadWrite | Access::Write) {
            return Err(WriteError::ReadOnly(def.name.clone()));
        }
        let tx = self
            .writers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(address.driver())
            .cloned()
            .ok_or_else(|| WriteError::NoDriver(address.driver().to_string()))?;
        let (reply, rx) = oneshot::channel();
        tx.send(WriteRequest { id, value, reply })
            .await
            .map_err(|_| WriteError::NoDriver(address.driver().to_string()))?;
        match tokio::time::timeout(WRITE_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(WriteError::Device("driver dropped the request".into())),
            Err(_) => Err(WriteError::Timeout),
        }
    }

    /// Applies an HMI command: set, toggle, increase/decrease by `value`, or
    /// pulse (write `value`, hold for [`PULSE`], then the resting value: the opposite
    /// for a bool, zero for a number).
    pub async fn write_mode(&self, id: TagId, value: Value, mode: WriteMode) -> Result<(), WriteError> {
        let current = self.get(id).value;
        let delta = || value.as_f64().ok_or_else(|| WriteError::BadValue(format!("{value} is not a number")));
        match mode {
            WriteMode::Set => self.write(id, value).await,
            WriteMode::Toggle => self.write(id, Value::Bool(!current.as_bool().unwrap_or(false))).await,
            WriteMode::Increase => self.write(id, Value::Float(current.as_f64().unwrap_or(0.0) + delta()?)).await,
            WriteMode::Decrease => self.write(id, Value::Float(current.as_f64().unwrap_or(0.0) - delta()?)).await,
            WriteMode::Pulse => {
                // Return to the resting value, not to whatever was read before: a
                // reset/start coil that was left on (lost write, device restart) must
                // still end up off, and a second pulse right after the first must not
                // "restore" the first pulse's value.
                let rest = match self.def(id).data_type {
                    DataType::Bool => Value::Bool(!value.as_bool().unwrap_or(true)),
                    ty => Value::default_for(ty),
                };
                self.write(id, value).await?;
                tokio::time::sleep(PULSE).await;
                self.write(id, rest).await
            }
        }
    }
}

#[cfg(test)]
mod tests;
