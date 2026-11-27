//! Helpers for Movicon's file formats: text decoding, XPO DataSet files,
//! connection strings and colours.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Reads a text file, honouring a UTF-8 or UTF-16 byte-order mark.
/// (`.references` files are UTF-16 LE; most others are UTF-8 with BOM.)
pub fn read_text(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let text = match bytes.as_slice() {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        rest => String::from_utf8_lossy(rest).into_owned(),
    };
    Ok(text)
}

fn utf16(bytes: &[u8], f: fn([u8; 2]) -> u16) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| f([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}

/// Parses XML text that may declare a non-UTF-8 encoding (already decoded).
pub fn parse_xml<'a>(text: &'a str, path: &Path) -> Result<roxmltree::Document<'a>> {
    let body = match text.trim_start().strip_prefix("<?xml") {
        Some(rest) => rest.split_once("?>").map(|(_, b)| b).unwrap_or(rest),
        None => text,
    };
    roxmltree::Document::parse(body).with_context(|| format!("parsing XML in {}", path.display()))
}

/// Direct child element by local name.
pub fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children().find(|n| n.is_element() && n.tag_name().name() == name)
}

/// Text of a direct child element, trimmed; `None` when absent, empty or `i:nil`.
pub fn child_text<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    child(node, name).and_then(|n| n.text()).map(str::trim).filter(|s| !s.is_empty())
}

/// One row of an XPO DataSet table: column name → text.
#[derive(Debug, Default, Clone)]
pub struct Row(HashMap<String, String>);

impl Row {
    /// Column value, `None` when missing or empty.
    pub fn get(&self, col: &str) -> Option<&str> {
        self.0.get(col).map(|s| s.trim()).filter(|s| !s.is_empty())
    }
    pub fn int(&self, col: &str) -> Option<i64> {
        self.get(col)?.parse().ok()
    }
    pub fn flag(&self, col: &str) -> bool {
        self.get(col) == Some("true")
    }
}

pub type Tables = HashMap<String, Vec<Row>>;

/// Reads a serialized ADO.NET DataSet (DevExpress XPO `InMemoryDataStore`):
/// an inline `xs:schema` followed by one element per row.
pub fn read_dataset(path: &Path) -> Result<Tables> {
    let text = read_text(path)?;
    let doc = parse_xml(&text, path)?;
    let mut tables: Tables = HashMap::new();
    for row in doc.root_element().children().filter(|n| n.is_element()) {
        let table = row.tag_name().name();
        if table == "schema" {
            continue;
        }
        let cols = row
            .children()
            .filter(|n| n.is_element())
            .map(|c| (c.tag_name().name().to_string(), c.text().unwrap_or("").to_string()))
            .collect();
        tables.entry(table.to_string()).or_default().push(Row(cols));
    }
    Ok(tables)
}

pub fn rows<'a>(tables: &'a Tables, name: &str) -> &'a [Row] {
    tables.get(name).map(Vec::as_slice).unwrap_or(&[])
}

/// `key=value;key=value` → lower-cased keys.
pub fn parse_conn_string(s: &str) -> HashMap<String, String> {
    s.split(';')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect()
}

/// `Flow Meters` → `flow-meters`.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "unnamed".into() } else { out }
}

/// Lower-cased GUID from a bare GUID, `ns=2;g=GUID` or `GUID|path|…`.
pub fn guid_of(s: &str) -> Option<String> {
    let s = s.split('|').next()?.trim();
    let s = s.rsplit_once("g=").map(|(_, g)| g).unwrap_or(s);
    let g = s.to_ascii_lowercase();
    let valid = g.len() == 36 && g.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    (valid && g != "00000000-0000-0000-0000-000000000000").then_some(g)
}

/// WPF colour → CSS. `#AARRGGBB` drops an opaque alpha; `{x:Null}` and
/// scRGB (`sc#…`) colours return `None`.
pub fn css_color(s: &str) -> Option<String> {
    let s = s.trim();
    if s.starts_with('{') || s.starts_with("sc#") || s.is_empty() {
        return None;
    }
    let Some(hex) = s.strip_prefix('#') else {
        return Some(s.to_string());
    };
    match hex.len() {
        8 => {
            let a = u8::from_str_radix(&hex[0..2], 16).ok()?;
            if a == 255 {
                Some(format!("#{}", &hex[2..]))
            } else {
                let c = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0);
                Some(format!("rgba({}, {}, {}, {:.3})", c(2), c(4), c(6), a as f64 / 255.0))
            }
        }
        _ => Some(s.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids() {
        assert_eq!(guid_of("ns=2;g=EBA96A81-4ea1-40c1-865c-77161b5027e3").as_deref(), Some("eba96a81-4ea1-40c1-865c-77161b5027e3"));
        assert_eq!(guid_of("eba96a81-4ea1-40c1-865c-77161b5027e3|2:Tags/2:X|ns=2;g=…").as_deref(), Some("eba96a81-4ea1-40c1-865c-77161b5027e3"));
        assert_eq!(guid_of("00000000-0000-0000-0000-000000000000||i=0|"), None);
    }

    #[test]
    fn colors() {
        assert_eq!(css_color("#FF3F3F3F").as_deref(), Some("#3F3F3F"));
        assert_eq!(css_color("#803F3F3F").as_deref(), Some("rgba(63, 63, 63, 0.502)"));
        assert_eq!(css_color("{x:Null}"), None);
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Flow Meters"), "flow-meters");
        assert_eq!(slug("Petrogas_IOServer"), "petrogas-ioserver");
    }
}
