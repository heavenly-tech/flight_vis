#![allow(dead_code)]

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Default)]
pub struct MailOutbox {
    inner: Arc<Mutex<Vec<CapturedMail>>>,
    send_hits: Arc<Mutex<HashMap<String, Vec<u64>>>>,
}

#[derive(Debug, Clone)]
pub struct CapturedMail {
    pub to: String,
    pub subject: String,
    pub body: String,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Clone)]
pub struct Mailer {
    pub smtp_host: Option<String>,
    pub smtp_from: String,
    pub public_url: String,
    bounce_secret: Option<Vec<u8>>,
    outbox: MailOutbox,
    per_email_limit: usize,
    per_ip_limit: usize,
    window_secs: u64,
    ip_hits: Arc<Mutex<HashMap<String, Vec<u64>>>>,
}

impl Mailer {
    pub fn from_env(outbox: MailOutbox) -> Self {
        let smtp_host = std::env::var("SMTP_HOST").ok().filter(|s| !s.is_empty());
        let smtp_from =
            std::env::var("SMTP_FROM").unwrap_or_else(|_| "noreply@heavenly.cl".to_string());
        let public_url = std::env::var("FLIGHT_CLUB_PUBLIC_URL")
            .unwrap_or_else(|_| "https://flight.heavenly.cl".to_string());
        let bounce_secret = std::env::var("POSTAL_WEBHOOK_SECRET_FILE")
            .ok()
            .and_then(|p| std::fs::read(p).ok())
            .or_else(|| {
                std::env::var("POSTAL_WEBHOOK_SECRET")
                    .ok()
                    .map(|s| s.into_bytes())
            });
        Self {
            smtp_host,
            smtp_from,
            public_url,
            bounce_secret,
            outbox,
            per_email_limit: 5,
            per_ip_limit: 20,
            window_secs: 3600,
            ip_hits: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn allow_send(&self, email: &str, ip: &str) -> bool {
        let now = now_secs();
        let cutoff = now.saturating_sub(self.window_secs);
        {
            let mut hits = self.outbox.send_hits.lock().unwrap();
            let entry = hits.entry(email.to_ascii_lowercase()).or_default();
            entry.retain(|t| *t > cutoff);
            if entry.len() >= self.per_email_limit {
                return false;
            }
            entry.push(now);
        }
        let mut ip_hits = self.ip_hits.lock().unwrap();
        let entry = ip_hits.entry(ip.to_string()).or_default();
        entry.retain(|t| *t > cutoff);
        if entry.len() >= self.per_ip_limit {
            return false;
        }
        entry.push(now);
        true
    }

    pub fn capture(&self, to: &str, subject: &str, body: &str) {
        self.outbox.inner.lock().unwrap().push(CapturedMail {
            to: to.to_string(),
            subject: subject.to_string(),
            body: body.to_string(),
        });
        if self.smtp_host.is_some() {
            eprintln!("mail: would smtp-send to {to}: {subject}");
        }
    }

    /// Send (capture) an email using locale catalogs (`mail.{stem}.*` keys).
    pub fn capture_localized(&self, to: &str, locale: &str, stem: &str, vars: &[(&str, &str)]) {
        let Some((subject, plain, _html)) = self.localized_parts(locale, stem, vars) else {
            return;
        };
        self.capture(to, &subject, &plain);
    }

    fn localized_parts(
        &self,
        locale: &str,
        stem: &str,
        vars: &[(&str, &str)],
    ) -> Option<(String, String, String)> {
        let cat = crate::i18n::global()?;
        let loc = if cat.is_known(locale) {
            locale.to_string()
        } else {
            cat.canonical.clone()
        };
        Some(cat.mail(&loc, stem, vars))
    }

    /// Capture when SMTP is unset; send plain+html when `SMTP_HOST` is configured.
    pub async fn send_localized(
        &self,
        to: &str,
        locale: &str,
        stem: &str,
        vars: &[(&str, &str)],
    ) {
        let Some((subject, plain, html)) = self.localized_parts(locale, stem, vars) else {
            return;
        };
        if self.smtp_host.is_none() {
            self.capture(to, &subject, &plain);
            return;
        }
        if let Err(e) = self.smtp_send(to, &subject, &plain, &html).await {
            eprintln!("mail: smtp send failed for {to}: {e}");
            self.capture(to, &subject, &plain);
        }
    }

    async fn smtp_send(
        &self,
        to: &str,
        subject: &str,
        plain: &str,
        html: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use lettre::message::{header::ContentType, MultiPart, SinglePart};
        use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
        let host = self
            .smtp_host
            .as_deref()
            .ok_or("smtp host missing")?;
        let from: lettre::message::Mailbox = self.smtp_from.parse()?;
        let to_mail: lettre::message::Mailbox = to.parse()?;
        let body = MultiPart::alternative()
            .singlepart(
                SinglePart::builder()
                    .header(ContentType::TEXT_PLAIN)
                    .body(plain.to_string()),
            )
            .singlepart(
                SinglePart::builder()
                    .header(ContentType::TEXT_HTML)
                    .body(html.to_string()),
            );
        let email = Message::builder()
            .from(from)
            .to(to_mail)
            .subject(subject)
            .multipart(body)?;
        let mailer = AsyncSmtpTransport::<Tokio1Executor>::relay(host)?.build();
        mailer.send(email).await?;
        Ok(())
    }

    pub fn verify_postal_signature(&self, body: &[u8], signature: &str) -> bool {
        let Some(secret) = &self.bounce_secret else {
            return false;
        };
        let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
        mac.update(body);
        let expected = hex::encode(mac.finalize().into_bytes());
        subtle::ConstantTimeEq::ct_eq(expected.as_bytes(), signature.as_bytes()).into()
    }

    #[cfg(test)]
    pub fn captured(&self) -> Vec<CapturedMail> {
        self.outbox.inner.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_rate_limit_per_email() {
        let mailer = Mailer::from_env(MailOutbox::default());
        for _ in 0..5 {
            assert!(mailer.allow_send("a@test", "1.2.3.4"));
        }
        assert!(!mailer.allow_send("a@test", "1.2.3.4"));
    }

    #[test]
    fn capture_localized_spanish_magic_link() {
        let dir = crate::i18n::Catalogs::resolve_dir(std::path::Path::new("dist"));
        let _ = crate::i18n::init_global(&dir);
        let mailer = Mailer::from_env(MailOutbox::default());
        mailer.capture_localized(
            "p@test",
            "es",
            "magicLink",
            &[("minutes", "15"), ("url", "https://example.invalid/x")],
        );
        let m = mailer.captured().pop().unwrap();
        assert!(m.subject.contains("flight_vis") || m.subject.contains("acceso"));
    }

    #[test]
    fn postal_signature_roundtrip() {
        std::env::set_var("POSTAL_WEBHOOK_SECRET", "postal-test-secret");
        let mailer = Mailer::from_env(MailOutbox::default());
        let body = br#"{"email":"x@test"}"#;
        let mut mac = HmacSha256::new_from_slice(b"postal-test-secret").unwrap();
        mac.update(body);
        let sig = hex::encode(mac.finalize().into_bytes());
        assert!(mailer.verify_postal_signature(body, &sig));
        assert!(!mailer.verify_postal_signature(body, &sig.replacen('a', "b", 1)));
        std::env::remove_var("POSTAL_WEBHOOK_SECRET");
    }
}
