//! `alarms.toml`: conditions on tags that operators must see and acknowledge.
//!
//! ```toml
//! [[alarm]]
//! name = "FM01_HighFlow"
//! tag = "FM_01_MassFlow"
//! when = "above"
//! limit = 120.0
//! deadband = 2.0
//! severity = "high"
//! message = "Gasoline mass flow above 120 mTon/hr"
//! area = "Manifold"
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alarm {
    pub name: String,
    pub tag: String,
    #[serde(flatten)]
    pub condition: AlarmCondition,
    #[serde(default)]
    pub severity: AlarmSeverity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<String>,
    /// Hysteresis applied when the condition clears (analog conditions).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub deadband: f64,
    /// The condition must hold this long before the alarm activates.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub delay_s: f64,
    /// Operators must acknowledge the alarm before it leaves the list.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub ack_required: bool,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "when", rename_all = "kebab-case")]
pub enum AlarmCondition {
    Above { limit: f64 },
    Below { limit: f64 },
    Equals { value: f64 },
    IsTrue,
    IsFalse,
    /// The tag's quality is bad (device not answering, decode error).
    BadQuality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AlarmSeverity {
    Low,
    #[default]
    Medium,
    High,
    Critical,
}

fn yes() -> bool {
    true
}
fn is_true(v: &bool) -> bool {
    *v
}
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}
