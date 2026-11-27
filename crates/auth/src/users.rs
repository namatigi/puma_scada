//! Users: who may sign in to the HMI and the studio, with which role, and
//! their password hashes.
//!
//! Kept in `users.toml` in the runtime's data folder, outside the project
//! folder, so the list never travels with the project (git, copies, imports)
//! and a studio project save cannot change it. Passwords are stored only as
//! Argon2id hashes (PHC strings); plain passwords are never written or logged.
//!
//! ```toml
//! [[user]]
//! name = "alice"
//! role = "admin"
//! password = "$argon2id$v=19$m=19456,t=2,p=1$…"
//! ```

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

/// Shortest password accepted for a new or changed password.
pub const MIN_PASSWORD_LEN: usize = 8;

/// What a signed-in user may do. Each role includes the ones before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Watch the HMI (values, trends, alarms) and open the studio read-only.
    Viewer,
    /// Viewer, plus operating from the HMI: write tags, acknowledge and shelve alarms.
    Operator,
    /// Operator, plus editing and saving the project in the studio.
    Engineer,
    /// Engineer, plus managing users.
    Admin,
}

impl Role {
    pub const ALL: [Role; 4] = [Role::Viewer, Role::Operator, Role::Engineer, Role::Admin];

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Operator => "operator",
            Role::Engineer => "engineer",
            Role::Admin => "admin",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.as_str().eq_ignore_ascii_case(s.trim()))
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One user as stored in `users.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct User {
    pub name: String,
    pub role: Role,
    /// Argon2id PHC string.
    pub password: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UsersFile {
    #[serde(rename = "user", default)]
    users: Vec<User>,
}

const HEADER: &str = "# Moviclone users (HMI and studio). Passwords are Argon2id hashes, never plain text.\n# Manage them in the studio (Users) or with `movi users`.\n\n";

/// The users file of a runtime: `<data>/users.toml`.
pub fn users_file(data_dir: &Path) -> PathBuf {
    data_dir.join("users.toml")
}

/// The users in `path`; an empty list when the file does not exist yet.
pub fn load(path: &Path) -> Result<Vec<User>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let file: UsersFile = toml::from_str(&text).map_err(|e| format!("{} is not a valid users file: {e}", path.display()))?;
    for u in &file.users {
        check_name(&u.name).map_err(|e| format!("{}: {e}", path.display()))?;
        PasswordHash::new(&u.password)
            .map_err(|_| format!("{}: user {:?} has no valid password hash", path.display(), u.name))?;
    }
    Ok(file.users)
}

/// Writes `users` to `path` atomically.
pub fn save(path: &Path, users: &[User]) -> Result<(), String> {
    let text = toml::to_string(&UsersFile { users: users.to_vec() }).map_err(|e| format!("cannot write users: {e}"))?;
    write_atomic(path, &format!("{HEADER}{text}")).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Writes through a temporary file and a rename, so a crash or a reader never
/// sees half a users file. Windows refuses the rename for a moment while
/// another process has the file open, so it is retried briefly.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().ok_or_else(|| std::io::Error::other("file has no parent folder"))?;
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        let mut last = None;
        for attempt in 0..8u64 {
            match std::fs::rename(&tmp, path) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    last = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(15 * (attempt + 1)));
                }
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| std::io::Error::other("rename failed")))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// User names: 1–64 letters, digits, `.`, `_` or `-`. Compared without regard to case.
pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("a user name has 1 to 64 characters".into());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return Err(format!("user name {name:?} may only use letters, digits, '.', '_' and '-'"));
    }
    Ok(())
}

pub fn check_password(password: &str) -> Result<(), String> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(format!("a password has at least {MIN_PASSWORD_LEN} characters"));
    }
    if password.len() > 1024 {
        return Err("a password has at most 1024 bytes".into());
    }
    Ok(())
}

/// Argon2id hash of `password` with a fresh random salt. Slow on purpose (tens of ms).
pub fn hash_password(password: &str) -> Result<String, String> {
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).map_err(|e| format!("no random source: {e}"))?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| format!("salt: {e}"))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("cannot hash the password: {e}"))
}

/// Whether `password` matches the stored `hash`.
pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
}

/// The index of `name` in `users`, ignoring case.
pub fn find(users: &[User], name: &str) -> Option<usize> {
    users.iter().position(|u| u.name.eq_ignore_ascii_case(name))
}

/// Number of administrators in `users`.
pub fn admins(users: &[User]) -> usize {
    users.iter().filter(|u| u.role == Role::Admin).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_verify_and_never_contain_the_password() {
        let h = hash_password("correct horse").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(!h.contains("correct horse"));
        assert!(verify_password("correct horse", &h));
        assert!(!verify_password("correct horsE", &h));
        assert!(!verify_password("correct horse", "not a hash"));
        assert_ne!(h, hash_password("correct horse").unwrap(), "salted");
    }

    #[test]
    fn file_round_trips_and_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = users_file(dir.path());
        assert!(load(&path).unwrap().is_empty());
        let users = vec![User { name: "alice".into(), role: Role::Operator, password: hash_password("password1").unwrap() }];
        save(&path, &users).unwrap();
        assert_eq!(load(&path).unwrap(), users);
        assert!(std::fs::read_to_string(&path).unwrap().contains("role = \"operator\""));
    }

    #[test]
    fn bad_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = users_file(dir.path());
        std::fs::write(&path, "[[user]]\nname = \"bob\"\nrole = \"admin\"\npassword = \"secret123\"\n").unwrap();
        assert!(load(&path).unwrap_err().contains("no valid password hash"));
        std::fs::write(&path, "[[user]]\nname = \"bob\"\nrole = \"root\"\npassword = \"x\"\n").unwrap();
        assert!(load(&path).is_err());
    }

    #[test]
    fn names_roles_and_passwords_are_checked() {
        assert!(check_name("alice.o-k_1").is_ok());
        for bad in ["", "a b", "ä", "x/y", &"a".repeat(65)] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
        assert_eq!(Role::parse(" Engineer "), Some(Role::Engineer));
        assert_eq!(Role::parse("operator"), Some(Role::Operator));
        assert_eq!(Role::parse("root"), None);
        assert!(Role::Admin > Role::Engineer && Role::Engineer > Role::Operator && Role::Operator > Role::Viewer);
        assert!(check_password("1234567").is_err());
        assert!(check_password("12345678").is_ok());
    }
}
