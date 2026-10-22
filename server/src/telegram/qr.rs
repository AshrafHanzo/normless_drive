//! QR login.
//!
//! Avoids phone numbers and codes entirely: Telegram shows a token, the
//! user scans it with a phone that is already signed in, and the account
//! is authorised. The desktop app offers the same thing, and it is the
//! more reliable path — no country-code mistakes, no waiting for a code
//! that may be delivered somewhere the user is not looking.
//!
//! The flow is three raw TL calls because grammers has no high-level
//! wrapper for it:
//!
//!   1. `auth.exportLoginToken` → a token to render as a QR code
//!   2. poll the same call until it answers `loginTokenSuccess`
//!   3. if it answers `loginTokenMigrateTo`, the account lives in another
//!      datacenter and `auth.importLoginToken` must be invoked there

use base64::Engine as _;
use grammers_client::Client;
use grammers_tl_types as tl;

use super::TelegramError;

/// What a poll of the login token produced.
pub enum QrState {
    /// Still waiting for a scan. Carries the token to display and the
    /// unix time it stops being valid.
    Waiting { token: Vec<u8>, expires: i32 },
    /// Scanned and accepted.
    Authorized,
    /// Telegram wants this handled in a different datacenter.
    Migrate { dc_id: i32, token: Vec<u8> },
}

/// Asks Telegram for a login token.
pub async fn export(
    client: &Client,
    api_id: i32,
    api_hash: &str,
) -> Result<QrState, TelegramError> {
    let result = client
        .invoke(&tl::functions::auth::ExportLoginToken {
            api_id,
            api_hash: api_hash.to_string(),
            // Accounts already signed in on this client, which there are
            // none of — this connection exists only to be authorised.
            except_ids: Vec::new(),
        })
        .await
        .map_err(|error| super::classify_public(&error))?;

    Ok(interpret(result))
}

/// Completes a login that Telegram redirected to another datacenter.
pub async fn import(client: &Client, token: Vec<u8>) -> Result<QrState, TelegramError> {
    let result = client
        .invoke(&tl::functions::auth::ImportLoginToken { token })
        .await
        .map_err(|error| super::classify_public(&error))?;

    Ok(interpret(result))
}

fn interpret(result: tl::enums::auth::LoginToken) -> QrState {
    match result {
        tl::enums::auth::LoginToken::Token(token) => QrState::Waiting {
            token: token.token,
            expires: token.expires,
        },
        tl::enums::auth::LoginToken::Success(_) => QrState::Authorized,
        tl::enums::auth::LoginToken::MigrateTo(migrate) => QrState::Migrate {
            dc_id: migrate.dc_id,
            token: migrate.token,
        },
    }
}

/// The URL a Telegram client expects to find inside the QR code.
///
/// Base64 **url-safe, unpadded** — Telegram rejects standard base64 here,
/// and the padding characters break the `tg://` URL.
pub fn login_url(token: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token);
    format!("tg://login?token={encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_url_is_url_safe_and_unpadded() {
        // Bytes chosen to produce '+' and '/' under standard base64 and a
        // length that would normally be padded — all three would break the
        // URL or be rejected by Telegram.
        let token = vec![0xfb, 0xff, 0xbe, 0x01, 0x02];
        let url = login_url(&token);

        assert!(url.starts_with("tg://login?token="));
        let encoded = url.trim_start_matches("tg://login?token=");
        assert!(!encoded.contains('+'), "standard base64 leaked into the URL");
        assert!(!encoded.contains('/'), "standard base64 leaked into the URL");
        assert!(!encoded.contains('='), "padding leaked into the URL");
    }

    #[test]
    fn an_empty_token_still_produces_a_well_formed_url() {
        assert_eq!(login_url(&[]), "tg://login?token=");
    }

    #[test]
    fn the_encoding_round_trips() {
        let token: Vec<u8> = (0u8..=255).collect();
        let url = login_url(&token);
        let encoded = url.trim_start_matches("tg://login?token=");
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .expect("should decode");
        assert_eq!(decoded, token);
    }
}
