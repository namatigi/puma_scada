//! Evaluation of an alarm condition against one tag sample, with deadband.

use moviclone_model::AlarmCondition;
use moviclone_tagdb::{Quality, Sample, Value};

/// Tolerance for `equals` on values that went through float conversions.
const EQUALS_EPSILON: f64 = 1e-9;

/// Whether the condition holds for `sample`.
///
/// `was_true` is the previous result: analog conditions only clear once the
/// value has moved `deadband` back past the limit, so a noisy signal sitting
/// on the limit does not flood the journal.
///
/// Returns `None` when the sample cannot decide (bad quality or no value for a
/// value condition): the alarm keeps its state rather than clearing because the
/// device stopped answering. Communication loss is what `bad-quality` alarms are for.
pub fn evaluate(cond: &AlarmCondition, deadband: f64, was_true: bool, sample: &Sample) -> Option<bool> {
    if let AlarmCondition::BadQuality = cond {
        return Some(sample.quality == Quality::Bad);
    }
    if sample.quality == Quality::Bad || sample.value == Value::Null {
        return None;
    }
    let deadband = deadband.abs();
    match cond {
        AlarmCondition::Above { limit } => {
            let v = number(&sample.value)?;
            Some(if was_true { v > limit - deadband } else { v > *limit })
        }
        AlarmCondition::Below { limit } => {
            let v = number(&sample.value)?;
            Some(if was_true { v < limit + deadband } else { v < *limit })
        }
        AlarmCondition::Equals { value } => {
            let v = number(&sample.value)?;
            Some((v - value).abs() <= EQUALS_EPSILON * value.abs().max(1.0))
        }
        AlarmCondition::IsTrue => sample.value.as_bool(),
        AlarmCondition::IsFalse => sample.value.as_bool().map(|b| !b),
        AlarmCondition::BadQuality => unreachable!("handled above"),
    }
}

fn number(v: &Value) -> Option<f64> {
    v.as_f64().filter(|f| f.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: Value) -> Sample {
        Sample { value: v, quality: Quality::Good, ts: 0 }
    }

    #[test]
    fn above_with_deadband() {
        let c = AlarmCondition::Above { limit: 100.0 };
        assert_eq!(evaluate(&c, 2.0, false, &s(Value::Float(100.0))), Some(false));
        assert_eq!(evaluate(&c, 2.0, false, &s(Value::Float(100.5))), Some(true));
        // Active: stays active until below limit - deadband.
        assert_eq!(evaluate(&c, 2.0, true, &s(Value::Float(98.5))), Some(true));
        assert_eq!(evaluate(&c, 2.0, true, &s(Value::Float(98.0))), Some(false));
    }

    #[test]
    fn below_with_deadband() {
        let c = AlarmCondition::Below { limit: 10.0 };
        assert_eq!(evaluate(&c, 1.0, false, &s(Value::Int(9))), Some(true));
        assert_eq!(evaluate(&c, 1.0, true, &s(Value::Float(10.5))), Some(true));
        assert_eq!(evaluate(&c, 1.0, true, &s(Value::Int(11))), Some(false));
    }

    #[test]
    fn equals_and_booleans() {
        let c = AlarmCondition::Equals { value: 3.0 };
        assert_eq!(evaluate(&c, 0.0, false, &s(Value::Int(3))), Some(true));
        assert_eq!(evaluate(&c, 0.0, false, &s(Value::Float(3.1))), Some(false));
        assert_eq!(evaluate(&AlarmCondition::IsTrue, 0.0, false, &s(Value::Bool(true))), Some(true));
        assert_eq!(evaluate(&AlarmCondition::IsFalse, 0.0, false, &s(Value::Bool(true))), Some(false));
        assert_eq!(evaluate(&AlarmCondition::IsTrue, 0.0, false, &s(Value::Text("x".into()))), None);
    }

    #[test]
    fn bad_quality_is_indeterminate_except_for_bad_quality_alarms() {
        let bad = Sample { value: Value::Null, quality: Quality::Bad, ts: 0 };
        let c = AlarmCondition::Above { limit: 1.0 };
        assert_eq!(evaluate(&c, 0.0, true, &bad), None);
        assert_eq!(evaluate(&AlarmCondition::BadQuality, 0.0, false, &bad), Some(true));
        assert_eq!(evaluate(&AlarmCondition::BadQuality, 0.0, true, &s(Value::Int(1))), Some(false));
    }
}
