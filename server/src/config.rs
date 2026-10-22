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
    /// The one email address that is made an active administrator when it
    /// registers. Everyone else lands on `pending` regardless of order.
    ///
    /// Without this the only workable rule is "whoever registers first wins",
    /// and the hostname is in public DNS — so a stranger who finds the site
    /// before the owner does would own it. Stored folded to lowercase to
    /// match how `users.email` is compared.
    pub bootstrap_admin_email: Option<String>,
    /// Password for the bootstrap administrator, used **once** to create that
    /// account at startup if it does not already exist.
    ///
    /// Never used to change an existing account's password: otherwise a
    /// stale value left in the environment would silently reset the admin
    /// password on every restart, undoing any later change.
    ///
    /// It sits in plaintext in the environment for as long as it is set, so
    /// the app asks to have it removed once the account exists.
    pub bootstrap_admin_password: Option<String>,
    /// Secret used to wrap a second copy of a Telegram session so share
    /// links keep working while their owner is signed out.
    ///
    /// Deliberately separate from the database: a stolen dump is useless
    /// without it. Absent means offline sharing is simply unavailable —
    /// that is a supported configuration, not an error.
    pub share_secret: Option<String>,
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

        let bootstrap_admin_email = env::var("BOOTSTRAP_ADMIN_EMAIL")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());

        Ok(Self {
            database_url: required("DATABASE_URL")?,
            bind_addr: env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string()),
            public_base_url,
            session_ttl_hours: optional_parsed("SESSION_TTL_HOURS", "number of hours", 24 * 14)?,
            cookie_secure: optional_parsed("COOKIE_SECURE", "boolean", true)?,
            bootstrap_admin_email,
            // Not trimmed: a leading or trailing space is a legitimate part
            // of a password, and silently stripping it would lock the owner
            // out of an account they created with one.
            bootstrap_admin_password: env::var("BOOTSTRAP_ADMIN_PASSWORD")
                .ok()
                .filter(|value| !value.is_empty()),
            // A short secret would be brute-forceable offline against a
            // stolen database, which is the one attack this is supposed to
            // withstand. Refuse rather than offer weak protection.
            share_secret: match env::var("SHARE_SECRET") {
                Ok(value) if value.trim().len() >= 32 => Some(value.trim().to_string()),
                Ok(value) if value.trim().is_empty() => None,
                Ok(_) => {
                    return Err(ConfigError::Invalid {
                        name: "SHARE_SECRET",
                        expected: "at least 32 characters",
                        value: "<redacted>".to_string(),
                    })
                }
                Err(_) => None,
            },
        })
    }

    pub fn offline_sharing_available(&self) -> bool {
        self.share_secret.is_some()
    }

    /// Whether this address should be granted admin on registration.
    ///
    /// Returns false for every address when no bootstrap email is set, so an
    /// unconfigured deployment creates no administrator at all rather than
    /// handing the role to whoever arrives first.
    pub fn is_bootstrap_admin(&self, email: &str) -> bool {
        self.bootstrap_admin_email
            .as_deref()
            .is_some_and(|configured| configured == email.trim().to_ascii_lowercase())
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

    #[test]
    fn only_the_configured_address_bootstraps_to_admin() {
        temp_env(
            &[
                ("PUBLIC_BASE_URL", "https://x.test"),
                ("DATABASE_URL", "postgres://x/y"),
                ("BOOTSTRAP_ADMIN_EMAIL", "  Ashraf@Normless.in "),
            ],
            || {
                let config = Config::from_env().unwrap();
                // Folded and trimmed on both sides, so the casing someone
                // happens to type at the signup form does not decide
                // whether they get the service.
                assert!(config.is_bootstrap_admin("ashraf@normless.in"));
                assert!(config.is_bootstrap_admin("ASHRAF@NORMLESS.IN"));
                assert!(!config.is_bootstrap_admin("someone@else.com"));
            },
        );
    }

    #[test]
    fn with_no_bootstrap_email_nobody_is_an_admin() {
        // The failure mode to avoid is a deployment that silently hands the
        // first visitor the keys. No configuration means no admin, and the
        // owner has to say who it is.
        temp_env(
            &[
                ("PUBLIC_BASE_URL", "https://x.test"),
                ("DATABASE_URL", "postgres://x/y"),
            ],
            || {
                let config = Config::from_env().unwrap();
                assert!(!config.is_bootstrap_admin("anyone@at.all"));
                assert!(!config.is_bootstrap_admin(""));
            },
        );
    }

    /// Environment variables are process-global, and the test harness runs
    /// tests on parallel threads — so without this every env test races
    /// every other one, clearing variables a neighbour just set. The
    /// failures that produces look like bugs in `Config` rather than in the
    /// tests, which is exactly how much time they waste.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Sets the given variables for the duration of `body`, clearing every
    /// variable this module reads first so tests do not inherit each other's
    /// state or the developer's shell.
    fn temp_env(vars: &[(&str, &str)], body: impl FnOnce()) {
        // A panicking test poisons the lock; the guard is still what we need,
        // and failing every later test because an earlier one failed would
        // just hide the original error.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        const ALL: [&str; 7] = [
            "DATABASE_URL",
            "BIND_ADDR",
            "PUBLIC_BASE_URL",
            "SESSION_TTL_HOURS",
            "COOKIE_SECURE",
            "BOOTSTRAP_ADMIN_EMAIL",
            "BOOTSTRAP_ADMIN_PASSWORD",
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
