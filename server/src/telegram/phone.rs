//! Phone login driven by raw TL rather than grammers' wrapper.
//!
//! grammers' `request_login_code` discards the part of Telegram's reply
//! that says *how* the code was sent, and that turned out to matter a
//! great deal: Telegram has thirteen delivery types and several of them
//! cannot be completed by a server at all. A code requested as
//! `FirebaseSms` needs Play Integrity attestation from a real Android
//! device; `FragmentSms` is delivered on fragment.com, not to the phone.
//!
//! With the wrapper, all of those look identical to success — Telegram
//! answers 200, grammers hands back a token, and the user waits forever
//! for a code that was never going to arrive. Reading the type lets the
//! app say what actually happened and point at QR instead.
//!
//! The two word-based types matter for a different reason: their codes are
//! not digits. Any UI that filters input to numbers makes them impossible
//! to enter.

use grammers_client::types::PasswordToken;
use grammers_client::Client;
use grammers_tl_types as tl;

use super::TelegramError;

/// How Telegram chose to deliver the code, in terms a user can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// Sent to the Telegram app on another signed-in device.
    App,
    Sms,
    Call,
    MissedCall { prefix: String },
    Email { pattern: String },
    /// Delivered through fragment.com rather than to the phone.
    Fragment { url: String },
    /// Needs Play Integrity attestation from a real Android device.
    FirebaseSms,
    /// The code is a word or phrase, not digits.
    Word,
    Phrase,
    /// Telegram wants an email set up before it will send anything.
    EmailSetupRequired,
    Unknown,
}

impl Delivery {
    /// Whether this app can actually complete a login of this type.
    pub fn usable(&self) -> bool {
        !matches!(
            self,
            Delivery::FirebaseSms | Delivery::Fragment { .. } | Delivery::EmailSetupRequired
        )
    }

    /// Whether the code is digits. Word and phrase codes are not, and an
    /// input that strips non-digits would silently destroy them.
    pub fn numeric(&self) -> bool {
        !matches!(self, Delivery::Word | Delivery::Phrase)
    }

    /// What to tell the user, phrased as where to look.
    pub fn describe(&self) -> String {
        match self {
            Delivery::App =>
                "Telegram sent the code to your Telegram app — look for a message from the \
                 official Telegram account, not an SMS.".into(),
            Delivery::Sms => "Telegram sent the code by SMS.".into(),
            Delivery::Call => "Telegram is calling you with the code.".into(),
            Delivery::MissedCall { prefix } => format!(
                "Telegram is placing a missed call. The code is the last digits of the \
                 calling number, which starts {prefix}."
            ),
            Delivery::Email { pattern } => {
                format!("Telegram sent the code to your email ({pattern}).")
            }
            Delivery::Word =>
                "Telegram sent a WORD, not a number. Type the word exactly as it appears.".into(),
            Delivery::Phrase =>
                "Telegram sent a PHRASE, not a number. Type it exactly as it appears.".into(),
            Delivery::Fragment { url } => format!(
                "Telegram is delivering this code through Fragment ({url}), which this app \
                 cannot read. Use the QR code method instead."
            ),
            Delivery::FirebaseSms =>
                "Telegram wants to verify this number through an official Android app, which a \
                 website cannot do. Use the QR code method instead — it works for any account."
                    .into(),
            Delivery::EmailSetupRequired =>
                "Telegram wants an email address set up on this account first. Use the QR code \
                 method instead.".into(),
            Delivery::Unknown =>
                "Telegram used a delivery method this app does not recognise. Use the QR code \
                 method instead.".into(),
        }
    }
}

fn classify_delivery(kind: &tl::enums::auth::SentCodeType) -> Delivery {
    use tl::enums::auth::SentCodeType as T;
    match kind {
        T::App(_) => Delivery::App,
        T::Sms(_) => Delivery::Sms,
        T::Call(_) => Delivery::Call,
        T::MissedCall(missed) => Delivery::MissedCall {
            prefix: missed.prefix.clone(),
        },
        T::EmailCode(email) => Delivery::Email {
            pattern: email.email_pattern.clone(),
        },
        T::SetUpEmailRequired(_) => Delivery::EmailSetupRequired,
        T::FragmentSms(fragment) => Delivery::Fragment {
            url: fragment.url.clone(),
        },
        T::FirebaseSms(_) => Delivery::FirebaseSms,
        T::SmsWord(_) => Delivery::Word,
        T::SmsPhrase(_) => Delivery::Phrase,
        _ => Delivery::Unknown,
    }
}

pub struct SentCode {
    pub phone_code_hash: String,
    pub delivery: Delivery,
    /// What Telegram would switch to if asked to resend, if anything.
    ///
    /// This is how an SMS is obtained when Telegram's first choice was to
    /// push the code into the app: there is no way to demand SMS up
    /// front, only to ask for the next method after the first.
    pub next: Option<NextMethod>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextMethod {
    Sms,
    Call,
    MissedCall,
    Other,
}

impl NextMethod {
    pub fn describe(self) -> &'static str {
        match self {
            NextMethod::Sms => "Send it by SMS instead",
            NextMethod::Call => "Call me with the code instead",
            NextMethod::MissedCall => "Send a missed call instead",
            NextMethod::Other => "Try another way",
        }
    }
}

fn classify_next(kind: &tl::enums::auth::CodeType) -> NextMethod {
    use tl::enums::auth::CodeType as C;
    match kind {
        C::Sms => NextMethod::Sms,
        C::Call => NextMethod::Call,
        C::MissedCall => NextMethod::MissedCall,
        _ => NextMethod::Other,
    }
}

fn request(api_id: i32, api_hash: &str, phone: &str) -> tl::functions::auth::SendCode {
    tl::functions::auth::SendCode {
        phone_number: phone.to_string(),
        api_id,
        api_hash: api_hash.to_string(),
        settings: tl::enums::CodeSettings::Settings(tl::types::CodeSettings {
            allow_flashcall: false,
            current_number: false,
            allow_app_hash: false,
            allow_missed_call: false,
            // Explicitly off. Firebase verification needs Play Integrity
            // from a real Android app, which a server cannot produce — so
            // there is no point inviting Telegram to choose it.
            allow_firebase: false,
            unknown_number: false,
            logout_tokens: None,
            token: None,
            app_sandbox: None,
        }),
    }
}

/// Reads the datacenter number out of a `PHONE_MIGRATE_n` error.
fn migrate_target(error: &grammers_client::InvocationError) -> Option<i32> {
    let grammers_client::InvocationError::Rpc(rpc) = error else {
        return None;
    };
    (rpc.code == 303 && rpc.name.contains("PHONE_MIGRATE"))
        .then_some(rpc.value)
        .flatten()
        .map(|value| value as i32)
}

/// Asks Telegram to send a login code, and reports how it chose to.
///
/// Takes the whole `Connection` rather than just the client because a
/// number belonging to another datacenter has to be retried there, and
/// that means moving the connection.
pub async fn send_code(
    connection: &super::Connection,
    api_id: i32,
    api_hash: &str,
    phone: &str,
) -> Result<SentCode, TelegramError> {
    let client: &Client = &connection.client;

    let result = match client.invoke(&request(api_id, api_hash, phone)).await {
        Ok(result) => result,
        Err(error) => {
            // Telegram shards accounts by datacenter. An Indian number
            // typically lives on DC 5 while a fresh connection starts on
            // DC 2, so this is the common case, not an edge case.
            let Some(dc_id) = migrate_target(&error) else {
                return Err(super::classify_public(&error));
            };
            connection.migrate_to(dc_id);
            client
                .invoke(&request(api_id, api_hash, phone))
                .await
                .map_err(|error| super::classify_public(&error))?
        }
    };

    match result {
        tl::enums::auth::SentCode::Code(code) => {
            let delivery = classify_delivery(&code.r#type);
            let next = code.next_type.as_ref().map(classify_next);
            tracing::info!(?delivery, ?next, "telegram chose a code delivery method");
            Ok(SentCode {
                phone_code_hash: code.phone_code_hash,
                delivery,
                next,
            })
        }
        // The account was authorised without a code at all. Rare, but it
        // does happen when a recent login token is still valid.
        tl::enums::auth::SentCode::Success(_) => Err(TelegramError::Other(
            "Telegram signed in without a code. Please try the QR method.".into(),
        )),
        tl::enums::auth::SentCode::PaymentRequired(_) => Err(TelegramError::Other(
            "Telegram is asking for payment to send a code to this number.".into(),
        )),
    }
}

/// Asks Telegram to send the code again by its next method.
///
/// When Telegram pushes a code into the app, someone not signed in
/// anywhere else never sees it — and there is no way to ask for SMS up
/// front. This is the only route to one.
pub async fn resend(
    client: &Client,
    phone: &str,
    phone_code_hash: &str,
) -> Result<SentCode, TelegramError> {
    let result = client
        .invoke(&tl::functions::auth::ResendCode {
            phone_number: phone.to_string(),
            phone_code_hash: phone_code_hash.to_string(),
            reason: None,
        })
        .await
        .map_err(|error| super::classify_public(&error))?;

    match result {
        tl::enums::auth::SentCode::Code(code) => {
            let delivery = classify_delivery(&code.r#type);
            let next = code.next_type.as_ref().map(classify_next);
            tracing::info!(?delivery, ?next, "telegram resent the code");
            Ok(SentCode {
                // Telegram may issue a new hash on resend; using the old
                // one afterwards fails with PHONE_CODE_INVALID, which
                // looks to the user like they mistyped.
                phone_code_hash: code.phone_code_hash,
                delivery,
                next,
            })
        }
        _ => Err(TelegramError::Other(
            "Telegram could not resend the code.".into(),
        )),
    }
}

pub enum SignIn {
    Done,
    NeedsPassword(Box<PasswordToken>),
}

/// Submits the code. Falls through to 2FA when the account has it.
pub async fn sign_in(
    client: &Client,
    phone: &str,
    phone_code_hash: &str,
    code: &str,
) -> Result<SignIn, TelegramError> {
    let result = client
        .invoke(&tl::functions::auth::SignIn {
            phone_number: phone.to_string(),
            phone_code_hash: phone_code_hash.to_string(),
            phone_code: Some(code.to_string()),
            email_verification: None,
        })
        .await;

    match result {
        Ok(_) => Ok(SignIn::Done),
        Err(error) => {
            let text = error.to_string();
            if text.contains("SESSION_PASSWORD_NEEDED") {
                // `PasswordToken::new` is public, so grammers' SRP
                // implementation can still be reused for the hard part.
                let password = client
                    .invoke(&tl::functions::account::GetPassword {})
                    .await
                    .map_err(|error| super::classify_public(&error))?;
                let tl::enums::account::Password::Password(password) = password;
                return Ok(SignIn::NeedsPassword(Box::new(PasswordToken::new(password))));
            }
            Err(super::classify_public(&error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_a_server_cannot_complete_are_marked_unusable() {
        // Each of these answers "code sent" and then never arrives. The
        // whole point of reading the type is to say so instead of leaving
        // someone staring at an empty inbox.
        assert!(!Delivery::FirebaseSms.usable());
        assert!(!Delivery::EmailSetupRequired.usable());
        assert!(!Delivery::Fragment { url: "https://fragment.com".into() }.usable());

        assert!(Delivery::App.usable());
        assert!(Delivery::Sms.usable());
        assert!(Delivery::Word.usable());
    }

    #[test]
    fn word_codes_are_flagged_as_not_numeric() {
        // The input used to strip non-digits, which would erase these
        // entirely and leave the user unable to submit anything.
        assert!(!Delivery::Word.numeric());
        assert!(!Delivery::Phrase.numeric());
        assert!(Delivery::App.numeric());
        assert!(Delivery::Sms.numeric());
    }

    #[test]
    fn every_description_points_somewhere_useful() {
        for delivery in [
            Delivery::App,
            Delivery::Sms,
            Delivery::FirebaseSms,
            Delivery::Word,
            Delivery::EmailSetupRequired,
            Delivery::Unknown,
        ] {
            let text = delivery.describe();
            assert!(text.ends_with('.'), "{text:?} is not a sentence");
            // Anything we cannot finish must name the way out.
            if !delivery.usable() {
                assert!(text.contains("QR"), "{text:?} offers no alternative");
            }
        }
    }
}
