//! Conversion between tag values and Modbus registers / bits.
//!
//! Multi-register values are big-endian with the high word first (the Modbus
//! convention and what the Petrogas flow meters use). Strings occupy 32 bytes
//! (16 registers) of ASCII, two characters per register, high byte first, padded
//! with NUL. 8-bit types live in the low byte of one register.
//!
//! The simulator uses the same functions, so the driver and the simulator can
//! never disagree about the byte order.

use moviclone_model::DataType;
use moviclone_tagdb::Value;
use std::fmt;

/// Bytes a string tag occupies on the device.
pub const STRING_BYTES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// Fewer registers than the data type needs.
    Short { ty: DataType, need: usize, got: usize },
    /// A float register pair holds NaN or infinity.
    NotFinite,
    /// The value cannot be represented in the tag's type.
    BadValue(String),
    /// Bits cannot hold anything but booleans.
    NotABit(DataType),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodecError::Short { ty, need, got } => write!(f, "{ty:?} needs {need} registers, got {got}"),
            CodecError::NotFinite => f.write_str("register value is not a finite number"),
            CodecError::BadValue(m) => write!(f, "invalid value: {m}"),
            CodecError::NotABit(ty) => write!(f, "{ty:?} cannot be stored in a single bit"),
        }
    }
}

impl std::error::Error for CodecError {}

/// Registers a value of `ty` occupies (booleans stored in a register use one).
pub fn register_count(ty: DataType) -> usize {
    ty.registers().unwrap_or(1) as usize
}

/// Decodes a value of type `ty` from the start of `words`.
pub fn decode_registers(ty: DataType, words: &[u16]) -> Result<Value, CodecError> {
    let need = register_count(ty);
    if words.len() < need {
        return Err(CodecError::Short { ty, need, got: words.len() });
    }
    let w = &words[..need];
    let u32_of = |w: &[u16]| (u32::from(w[0]) << 16) | u32::from(w[1]);
    let u64_of = |w: &[u16]| w.iter().fold(0u64, |acc, &x| (acc << 16) | u64::from(x));
    Ok(match ty {
        DataType::Bool => Value::Bool(w[0] != 0),
        DataType::I8 => Value::Int(i64::from(w[0] as u8 as i8)),
        DataType::U8 => Value::Int(i64::from(w[0] as u8)),
        DataType::I16 => Value::Int(i64::from(w[0] as i16)),
        DataType::U16 => Value::Int(i64::from(w[0])),
        DataType::I32 => Value::Int(i64::from(u32_of(w) as i32)),
        DataType::U32 => Value::Int(i64::from(u32_of(w))),
        DataType::I64 => Value::Int(u64_of(w) as i64),
        // The tag store holds integers as i64; values above i64::MAX are
        // reported as a float rather than wrapped into negative numbers.
        DataType::U64 => match i64::try_from(u64_of(w)) {
            Ok(i) => Value::Int(i),
            Err(_) => Value::Float(u64_of(w) as f64),
        },
        DataType::F32 => finite(f64::from(f32::from_bits(u32_of(w))))?,
        DataType::F64 => finite(f64::from_bits(u64_of(w)))?,
        DataType::String => Value::Text(decode_string(w)),
    })
}

fn finite(f: f64) -> Result<Value, CodecError> {
    if f.is_finite() {
        Ok(Value::Float(f))
    } else {
        Err(CodecError::NotFinite)
    }
}

fn decode_string(words: &[u16]) -> String {
    words
        .iter()
        .flat_map(|w| w.to_be_bytes())
        .take_while(|&b| b != 0)
        .map(|b| if b.is_ascii() && !b.is_ascii_control() { b as char } else { '?' })
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Encodes `value` (already coerced to `ty` or convertible to it) as registers.
pub fn encode_registers(ty: DataType, value: &Value) -> Result<Vec<u16>, CodecError> {
    let v = value.coerce(ty).map_err(CodecError::BadValue)?;
    let int = || match &v {
        Value::Int(i) => Ok(*i),
        Value::Bool(b) => Ok(i64::from(*b)),
        other => Err(CodecError::BadValue(format!("{other} is not an integer"))),
    };
    let float = || v.as_f64().ok_or_else(|| CodecError::BadValue(format!("{v} is not a number")));
    let split32 = |x: u32| vec![(x >> 16) as u16, x as u16];
    let split64 = |x: u64| vec![(x >> 48) as u16, (x >> 32) as u16, (x >> 16) as u16, x as u16];
    Ok(match ty {
        DataType::Bool => vec![u16::from(v.as_bool().unwrap_or(false))],
        DataType::I8 => vec![u16::from(int()? as i8 as u8)],
        DataType::U8 => vec![u16::from(int()? as u8)],
        DataType::I16 => vec![int()? as i16 as u16],
        DataType::U16 => vec![int()? as u16],
        DataType::I32 => split32(int()? as i32 as u32),
        DataType::U32 => split32(int()? as u32),
        DataType::I64 => split64(int()? as u64),
        DataType::U64 => split64(int()? as u64),
        DataType::F32 => split32((float()? as f32).to_bits()),
        DataType::F64 => split64(float()?.to_bits()),
        DataType::String => encode_string(&v.to_string_lossy())?,
    })
}

fn encode_string(s: &str) -> Result<Vec<u16>, CodecError> {
    if !s.is_ascii() {
        return Err(CodecError::BadValue("only ASCII text can be written to the device".into()));
    }
    if s.len() > STRING_BYTES {
        return Err(CodecError::BadValue(format!("text is longer than {STRING_BYTES} characters")));
    }
    let mut bytes = [0u8; STRING_BYTES];
    bytes[..s.len()].copy_from_slice(s.as_bytes());
    Ok(bytes.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect())
}

/// Decodes a coil / discrete input for a tag of type `ty`.
pub fn decode_bit(ty: DataType, bit: bool) -> Value {
    match ty {
        DataType::Bool | DataType::String => Value::Bool(bit),
        DataType::F32 | DataType::F64 => Value::Float(if bit { 1.0 } else { 0.0 }),
        _ => Value::Int(i64::from(bit)),
    }
}

/// Encodes a value for a coil write.
pub fn encode_bit(ty: DataType, value: &Value) -> Result<bool, CodecError> {
    if ty == DataType::String {
        return Err(CodecError::NotABit(ty));
    }
    value
        .as_bool()
        .ok_or_else(|| CodecError::BadValue(format!("{value} is not a boolean")))
}

trait Lossy {
    fn to_string_lossy(&self) -> String;
}

impl Lossy for Value {
    fn to_string_lossy(&self) -> String {
        match self {
            Value::Text(s) => s.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_is_big_endian_high_word_first() {
        // 123.5f32 = 0x42F7_0000
        assert_eq!(encode_registers(DataType::F32, &Value::Float(123.5)).unwrap(), vec![0x42F7, 0x0000]);
        assert_eq!(decode_registers(DataType::F32, &[0x42F7, 0x0000]).unwrap(), Value::Float(123.5));
    }

    #[test]
    fn integers_round_trip_with_sign() {
        for (ty, v) in [
            (DataType::I16, -2i64),
            (DataType::U16, 65535),
            (DataType::I32, -100_000),
            (DataType::U32, 4_000_000_000),
            (DataType::I64, -9_000_000_000_000),
            (DataType::U64, 9_000_000_000_000),
            (DataType::I8, -5),
            (DataType::U8, 200),
        ] {
            let regs = encode_registers(ty, &Value::Int(v)).unwrap();
            assert_eq!(regs.len(), register_count(ty), "{ty:?}");
            assert_eq!(decode_registers(ty, &regs).unwrap(), Value::Int(v), "{ty:?}");
        }
        assert_eq!(encode_registers(DataType::I32, &Value::Int(-1)).unwrap(), vec![0xFFFF, 0xFFFF]);
        assert_eq!(encode_registers(DataType::U32, &Value::Int(0x0001_0002)).unwrap(), vec![1, 2]);
    }

    #[test]
    fn f64_round_trips() {
        let regs = encode_registers(DataType::F64, &Value::Float(-1234.5678)).unwrap();
        assert_eq!(regs.len(), 4);
        assert_eq!(decode_registers(DataType::F64, &regs).unwrap(), Value::Float(-1234.5678));
    }

    #[test]
    fn huge_u64_decodes_as_float() {
        let v = decode_registers(DataType::U64, &[0xFFFF; 4]).unwrap();
        assert_eq!(v, Value::Float(u64::MAX as f64));
    }

    #[test]
    fn non_finite_float_is_an_error() {
        assert_eq!(decode_registers(DataType::F32, &[0x7FC0, 0]), Err(CodecError::NotFinite));
    }

    #[test]
    fn short_input_is_an_error() {
        assert!(matches!(decode_registers(DataType::F32, &[1]), Err(CodecError::Short { need: 2, got: 1, .. })));
    }

    #[test]
    fn strings_are_32_bytes_of_ascii() {
        let regs = encode_registers(DataType::String, &Value::Text("AB C".into())).unwrap();
        assert_eq!(regs.len(), 16);
        assert_eq!(&regs[..3], &[0x4142, 0x2043, 0]);
        assert_eq!(decode_registers(DataType::String, &regs).unwrap(), Value::Text("AB C".into()));
        assert!(encode_registers(DataType::String, &Value::Text("x".repeat(33))).is_err());
        assert!(encode_registers(DataType::String, &Value::Text("é".into())).is_err());
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        assert!(encode_registers(DataType::U16, &Value::Int(70_000)).is_err());
        assert!(encode_registers(DataType::I16, &Value::Float(1.5)).is_err());
    }

    #[test]
    fn bits() {
        assert_eq!(decode_bit(DataType::Bool, true), Value::Bool(true));
        assert_eq!(decode_bit(DataType::U16, true), Value::Int(1));
        assert!(encode_bit(DataType::Bool, &Value::Int(1)).unwrap());
        assert!(!encode_bit(DataType::Bool, &Value::Text("off".into())).unwrap());
        assert!(encode_bit(DataType::String, &Value::Bool(true)).is_err());
    }

    #[test]
    fn bool_in_register() {
        assert_eq!(decode_registers(DataType::Bool, &[7]).unwrap(), Value::Bool(true));
        assert_eq!(encode_registers(DataType::Bool, &Value::Bool(true)).unwrap(), vec![1]);
    }
}
