//! Sending and checking SMS codes. Production: Twilio Verify, which also keeps the codes. Development: one fixed
//! code, nothing sent.

use crate::config::SmsConfig;
use std::future::Future;
use std::pin::Pin;

pub type Fut<T> = Pin<Box<dyn Future<Output = T> + Send>>;

pub trait Sms: Send + Sync {
    /// Sends a code to `+{phone}`.
    fn start(&self, phone: String) -> Fut<Result<(), String>>;
    /// Whether `code` is the one sent to `+{phone}`.
    fn check(&self, phone: String, code: String) -> Fut<Result<bool, String>>;
}

pub fn from_config(config: &SmsConfig) -> Box<dyn Sms> {
    match config.clone() {
        SmsConfig::Dev { code } => Box::new(Dev { code }),
        SmsConfig::Twilio { account_sid, auth_token, service_sid } => {
            Box::new(Twilio { client: reqwest::Client::new(), account_sid, auth_token, service_sid })
        }
    }
}

struct Dev {
    code: String,
}

impl Sms for Dev {
    fn start(&self, _: String) -> Fut<Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn check(&self, _: String, code: String) -> Fut<Result<bool, String>> {
        let ok = code == self.code;
        Box::pin(async move { Ok(ok) })
    }
}

struct Twilio {
    client: reqwest::Client,
    account_sid: String,
    auth_token: String,
    service_sid: String,
}

impl Twilio {
    fn post(&self, what: &str, form: Vec<(&'static str, String)>) -> Fut<Result<serde_json::Value, String>> {
        let request = self
            .client
            .post(format!("https://verify.twilio.com/v2/Services/{}/{what}", self.service_sid))
            .basic_auth(&self.account_sid, Some(&self.auth_token))
            .form(&form);
        Box::pin(async move {
            let response = request.send().await.map_err(|e| e.to_string())?;
            let status = response.status();
            let body: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
            if status.is_success() {
                Ok(body)
            } else {
                Err(format!("twilio {status}: {}", body["message"]))
            }
        })
    }
}

impl Sms for Twilio {
    fn start(&self, phone: String) -> Fut<Result<(), String>> {
        let sent = self.post("Verifications", vec![("To", format!("+{phone}")), ("Channel", "sms".into())]);
        Box::pin(async move { sent.await.map(|_| ()) })
    }

    fn check(&self, phone: String, code: String) -> Fut<Result<bool, String>> {
        let checked = self.post("VerificationCheck", vec![("To", format!("+{phone}")), ("Code", code)]);
        Box::pin(async move {
            match checked.await {
                Ok(body) => Ok(body["status"] == "approved"),
                // Twilio answers 404 when no verification is pending (expired, used, never started).
                Err(error) if error.starts_with("twilio 404") => Ok(false),
                Err(error) => Err(error),
            }
        })
    }
}
