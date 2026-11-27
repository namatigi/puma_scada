//! Compact binary encoding of samples and logger rows for the journal.
//!
//! A historian can hold hundreds of millions of samples, so values are stored
//! as a type byte plus payload (10 bytes for a float with quality) rather than
//! JSON. Layout (little endian):
//!
//! ```text
//! sample = type:u8 payload quality:u8
//!   type 0 null | 1 false | 2 true | 3 int (i64) | 4 float (f64) | 5 text (u32 len + UTF-8)
//! row    = count:u16 sample*count
//! ```

use anyhow::{bail, Context};
use moviclone_tagdb::{Quality, Value};

pub fn encode_sample(value: &Value, quality: Quality, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.push(0),
        Value::Bool(false) => out.push(1),
        Value::Bool(true) => out.push(2),
        Value::Int(i) => {
            out.push(3);
            out.extend_from_slice(&i.to_le_bytes());
        }
        Value::Float(f) => {
            out.push(4);
            out.extend_from_slice(&f.to_le_bytes());
        }
        Value::Text(s) => {
            out.push(5);
            out.extend_from_slice(&(s.len() as u32).to_le_bytes());
            out.extend_from_slice(s.as_bytes());
        }
    }
    out.push(match quality {
        Quality::Good => 0,
        Quality::Uncertain => 1,
        Quality::Bad => 2,
    });
}

/// Decodes one sample, returning it and the remaining bytes.
pub fn decode_sample(buf: &[u8]) -> anyhow::Result<((Value, Quality), &[u8])> {
    let (&ty, rest) = buf.split_first().context("empty sample")?;
    let (value, rest) = match ty {
        0 => (Value::Null, rest),
        1 => (Value::Bool(false), rest),
        2 => (Value::Bool(true), rest),
        3 => {
            let (b, rest) = take::<8>(rest)?;
            (Value::Int(i64::from_le_bytes(b)), rest)
        }
        4 => {
            let (b, rest) = take::<8>(rest)?;
            (Value::Float(f64::from_le_bytes(b)), rest)
        }
        5 => {
            let (b, rest) = take::<4>(rest)?;
            let len = u32::from_le_bytes(b) as usize;
            if rest.len() < len {
                bail!("text sample truncated");
            }
            let (text, rest) = rest.split_at(len);
            (Value::Text(String::from_utf8(text.to_vec()).context("text sample is not UTF-8")?), rest)
        }
        other => bail!("unknown sample type {other}"),
    };
    let (&q, rest) = rest.split_first().context("sample without quality")?;
    let quality = match q {
        0 => Quality::Good,
        1 => Quality::Uncertain,
        2 => Quality::Bad,
        other => bail!("unknown quality {other}"),
    };
    Ok(((value, quality), rest))
}

pub fn encode_row(cells: &[(Value, Quality)], out: &mut Vec<u8>) {
    let n = cells.len().min(u16::MAX as usize);
    out.extend_from_slice(&(n as u16).to_le_bytes());
    for (v, q) in &cells[..n] {
        encode_sample(v, *q, out);
    }
}

pub fn decode_row(buf: &[u8]) -> anyhow::Result<Vec<(Value, Quality)>> {
    let (b, mut rest) = take::<2>(buf)?;
    let n = u16::from_le_bytes(b) as usize;
    let mut cells = Vec::with_capacity(n);
    for _ in 0..n {
        let (cell, r) = decode_sample(rest)?;
        cells.push(cell);
        rest = r;
    }
    Ok(cells)
}

fn take<const N: usize>(buf: &[u8]) -> anyhow::Result<([u8; N], &[u8])> {
    if buf.len() < N {
        bail!("record truncated");
    }
    let (head, rest) = buf.split_at(N);
    let mut out = [0u8; N];
    out.copy_from_slice(head);
    Ok((out, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_type() {
        let cells = vec![
            (Value::Null, Quality::Bad),
            (Value::Bool(true), Quality::Good),
            (Value::Bool(false), Quality::Uncertain),
            (Value::Int(-42), Quality::Good),
            (Value::Float(131.25), Quality::Good),
            (Value::Text("Jet A-1 ✓".into()), Quality::Good),
        ];
        let mut buf = Vec::new();
        encode_row(&cells, &mut buf);
        assert_eq!(decode_row(&buf).unwrap(), cells);
        let mut one = Vec::new();
        encode_sample(&Value::Float(1.5), Quality::Good, &mut one);
        assert_eq!(one.len(), 10);
    }

    #[test]
    fn rejects_truncated_input() {
        let mut buf = Vec::new();
        encode_sample(&Value::Text("hello".into()), Quality::Good, &mut buf);
        assert!(decode_sample(&buf[..buf.len() - 2]).is_err());
        assert!(decode_sample(&[9, 0]).is_err());
        assert!(decode_row(&[3, 0, 0]).is_err());
    }
}
