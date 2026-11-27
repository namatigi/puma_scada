//! Item names that become file names.
//!
//! A screen name ends up as `screens/<name>.toml`, so it must never be able to
//! leave that folder or collide with another screen through `file_stem_for`'s
//! replacement of unusual characters. Only names that `file_stem_for` maps to
//! themselves are accepted, which makes the name -> file mapping one-to-one.

/// Longest accepted screen name; Windows paths get unwieldy well before 255.
pub const MAX_NAME_LEN: usize = 100;

/// Names Windows reserves for devices, whatever the extension.
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1",
    "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Checks a screen name taken from a URL or a request body.
/// Returns a message an engineer can act on when it is refused.
pub fn check_screen_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("screen name is empty".into());
    }
    if name.len() > MAX_NAME_LEN {
        return Err(format!("screen name is longer than {MAX_NAME_LEN} characters"));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
        return Err(format!("screen name {name:?} may only contain letters, digits, '-', '_' and '.'"));
    }
    if name.starts_with('.') || name.ends_with('.') || name.contains("..") {
        return Err(format!("screen name {name:?} must not start or end with '.' or contain '..'"));
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        return Err(format!("screen name {name:?} is reserved by Windows"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::check_screen_name;

    #[test]
    fn ordinary_names_pass() {
        for ok in ["Manifold", "FM_01", "FM01_Trend", "Page-2", "v1.2"] {
            assert!(check_screen_name(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn traversal_and_odd_names_fail() {
        for bad in [
            "", "..", ".", "../x", "..\\x", "a/b", "a\\b", "c:x", "x\0", ".hidden", "x.", "a..b", "with space",
            "CON", "con.toml", "Lpt1", "Ünïcode", "%2e%2e",
        ] {
            assert!(check_screen_name(bad).is_err(), "{bad:?}");
        }
        assert!(check_screen_name(&"a".repeat(101)).is_err());
    }

    #[test]
    fn accepted_names_map_to_themselves() {
        for ok in ["Manifold", "FM_01", "a-b.c"] {
            assert_eq!(moviclone_model::io::file_stem_for(ok), ok);
        }
    }
}
