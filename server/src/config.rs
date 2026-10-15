//! Configuration, read once from the environment at startup.
//!
//! Everything fails loudly at boot rather than at first use: a service that
//! starts happily and then 500s on the first login because a variable was
//! missing is much worse to diagnose than one that refuses to start.

use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: String,
    /// Origin links are built from, e.g. `https://drive.normless.in`. Not
    /// derived from the request Host header — that is attacker-controlled and
    /// would let someone mint share links pointing at their own domain.
    pub public_base_url: String,
    pub session_ttl_hours: i64,
    /// Set `true` behind TLS so the session cookie carries `Secure`. Only
    /// false for local development over plain HTTP.
    pub cookie_secure: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0} is not set")]
    Missing(&'static str),
    #[error("{name} is not a valid {expected}: {value}")]
    Invalid {
        name: &'static str,
        expected: &'static str,
        value: String,
    },
}

fn required(name: &'static str) -> Result<String, ConfigError> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or(ConfigError::Missing(name))
}

fn optional_parsed<T: std::str::FromStr>(
    name: &'static str,
    expected: &'static str,
    default: T,
) -> Result<T, ConfigError> {
    match env::var(name) {
        Err(_) => Ok(default),
        Ok(raw) if raw.trim().is_empty() => Ok(default),
        Ok(raw) => raw.trim().parse().map_err(|_| ConfigError::Invalid {
            name,
            expected,
            value: raw,
        }),
    }
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let public_base_url = required("PUBLIC_BASE_URL")?;
        let public_base_url = public_base_url.trim_end_matches('/').to_string();

        Ok(Self {
            database_url: required("DATABASE_URL")?,
            bind_addr: env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string()),
            public_base_url,
            session_ttl_hours: optional_parsed("SESSION_TTL_HOURS", "number of hours", 24 * 14)?,
            cookie_secure: optional_parsed("COOKIE_SECURE", "boolean", true)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trailing_slash_is_trimmed_from_the_base_url() {
        // Share links are built by appending "/s/{token}", so a trailing
        // slash here would produce "https://host//s/abc".
        temp_env(
            &[
                ("PUBLIC_BASE_URL", "https://drive.normless.in/"),
                ("DATABASE_URL", "postgres://x/y"),
            ],
            || {
                let config = Config::from_env().expect("should parse");
                assert_eq!(config.public_base_url, "https://drive.normless.in");
            },
        );
    }

    #[test]
    fn a_missing_required_variable_names_itself() {
        temp_env(&[("PUBLIC_BASE_URL", "https://x.test")], || {
            let error = Config::from_env().expect_err("should refuse to start");
            assert!(error.to_string().contains("DATABASE_URL"), "got: {error}");
        });
    }

    #[test]
    fn a_blank_variable_counts_as_missing() {
        // An empty DATABASE_URL in a systemd unit is a far more common
        // mistake than an absent one, and must not be treated as present.
        temp_env(
            &[("PUBLIC_BASE_URL", "https://x.test"), ("DATABASE_URL", "   ")],
            || {
                assert!(Config::from_env().is_err());
            },
        );
    }

    #[test]
    fn an_unparseable_number_is_rejected_rather_than_defaulted() {
        temp_env(
            &[
                ("PUBLIC_BASE_URL", "https://x.test"),
                ("DATABASE_URL", "postgres://x/y"),
                ("SESSION_TTL_HOURS", "banana"),
            ],
            || {
                let error = Config::from_env().expect_err("should refuse to start");
                assert!(error.to_string().contains("SESSION_TTL_HOURS"), "got: {error}");
            },
        );
    }

    /// Sets the given variables for the duration of `body`, clearing every
    /// variable this module reads first so tests do not inherit each other's
    /// state or the developer's shell.
    fn temp_env(vars: &[(&str, &str)], body: impl FnOnce()) {
        const ALL: [&str; 5] = [
            "DATABASE_URL",
            "BIND_ADDR",
            "PUBLIC_BASE_URL",
            "SESSION_TTL_HOURS",
            "COOKIE_SECURE",
        ];
        for name in ALL {
            env::remove_var(name);
        }
        for (name, value) in vars {
            env::set_var(name, value);
        }
        body();
        for name in ALL {
            env::remove_var(name);
        }
    }
}
