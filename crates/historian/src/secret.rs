//! Resolution of `secret://name` references.
//!
//! `secret://petrogas-ioserver` is read from the environment variable
//! `MOVICLONE_SECRET_PETROGAS_IOSERVER` (upper case, `-` and `.` become `_`).
//! Values are never logged; errors name the variable, not its content.

use moviclone_model::SecretRef;

pub fn env_var_name(secret: &SecretRef) -> String {
    let name: String = secret
        .name()
        .chars()
        .map(|c| match c {
            '-' | '.' => '_',
            c => c.to_ascii_uppercase(),
        })
        .collect();
    format!("MOVICLONE_SECRET_{name}")
}

pub fn resolve(secret: &SecretRef) -> anyhow::Result<String> {
    let var = env_var_name(secret);
    match std::env::var(&var) {
        Ok(v) if !v.is_empty() => Ok(v),
        Ok(_) => anyhow::bail!("{secret} resolves to an empty value (environment variable {var})"),
        Err(_) => anyhow::bail!("{secret} is not set: provide it in the environment variable {var}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_names_to_env_vars() {
        let s = SecretRef::new("petrogas-ioserver.main").unwrap();
        assert_eq!(env_var_name(&s), "MOVICLONE_SECRET_PETROGAS_IOSERVER_MAIN");
    }

    #[test]
    fn resolves_from_env_and_never_echoes_value() {
        let s = SecretRef::new("hist-test-secret").unwrap();
        std::env::remove_var("MOVICLONE_SECRET_HIST_TEST_SECRET");
        let err = resolve(&s).unwrap_err().to_string();
        assert!(err.contains("MOVICLONE_SECRET_HIST_TEST_SECRET"));
        std::env::set_var("MOVICLONE_SECRET_HIST_TEST_SECRET", "s3cr3t");
        assert_eq!(resolve(&s).unwrap(), "s3cr3t");
        std::env::remove_var("MOVICLONE_SECRET_HIST_TEST_SECRET");
    }
}
