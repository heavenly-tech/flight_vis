#![allow(dead_code)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

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

#[derive(Clone, Debug)]
pub struct MailConfig {
    pub smtp_host: Option<String>,
    pub smtp_port: u16,
    pub smtp_password: Option<String>,
    pub smtp_from: String,
    pub public_url: String,
    pub postal_public_key_pem: Option<String>,
}

impl MailConfig {
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok().filter(|s| !s.is_empty()))
    }

    pub fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Self {
        let smtp_host = lookup("SMTP_HOST");
        let smtp_port = lookup("SMTP_PORT")
            .and_then(|p| p.parse().ok())
            .unwrap_or(25);
        let smtp_password = read_smtp_password(&mut lookup);
        let smtp_from = lookup("SMTP_FROM").unwrap_or_else(|| "noreply@heavenly.cl".to_string());
        let public_url = lookup("FLIGHT_CLUB_PUBLIC_URL")
            .unwrap_or_else(|| "https://flight.heavenly.cl".to_string());
        let postal_public_key_pem = lookup("POSTAL_WEBHOOK_PUBLIC_KEY_FILE").and_then(|path| {
            std::fs::read_to_string(&path)
                .ok()
                .filter(|s| !s.is_empty())
        });
        Self {
            smtp_host,
            smtp_port,
            smtp_password,
            smtp_from,
            public_url,
            postal_public_key_pem,
        }
    }
}

pub(crate) fn read_smtp_password(
    lookup: &mut impl FnMut(&str) -> Option<String>,
) -> Option<String> {
    if let Some(path) = lookup("SMTP_PASS_FILE") {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            let trimmed = raw.trim_end_matches(['\n', '\r']).to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    lookup("SMTP_PASS")
}

#[derive(Clone)]
pub struct Mailer {
    pub smtp_host: Option<String>,
    smtp_port: u16,
    smtp_password: Option<String>,
    pub smtp_from: String,
    pub public_url: String,
    postal_public_key_pem: Option<String>,
    outbox: MailOutbox,
    per_email_limit: usize,
    per_ip_limit: usize,
    window_secs: u64,
    ip_hits: Arc<Mutex<HashMap<String, Vec<u64>>>>,
}

impl Mailer {
    pub fn from_env(outbox: MailOutbox) -> Self {
        Self::from_config(MailConfig::from_env(), outbox)
    }

    pub fn from_config(config: MailConfig, outbox: MailOutbox) -> Self {
        Self {
            smtp_host: config.smtp_host,
            smtp_port: config.smtp_port,
            smtp_password: config.smtp_password,
            smtp_from: config.smtp_from,
            public_url: config.public_url,
            postal_public_key_pem: config.postal_public_key_pem,
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
    pub async fn send_localized(&self, to: &str, locale: &str, stem: &str, vars: &[(&str, &str)]) {
        let Some((subject, plain, html)) = self.localized_parts(locale, stem, vars) else {
            return;
        };
        if self.smtp_host.is_none() {
            self.capture(to, &subject, &plain);
            return;
        }
        if let Err(e) = self.smtp_send(to, &subject, &plain, &html).await {
            eprintln!("mail: smtp send failed for {to}: {e}");
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
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
        let host = self.smtp_host.as_deref().ok_or("smtp host missing")?;
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
        // Postal on the internal network: port 25, AUTH password = server credential key (username ignored).
        let mut builder = if self.smtp_port == 25 {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)?
        };
        builder = builder.port(self.smtp_port);
        if let Some(pass) = &self.smtp_password {
            builder = builder.credentials(Credentials::new(String::new(), pass.clone()));
        }
        let mailer = builder.build();
        mailer.send(email).await?;
        Ok(())
    }

    /// Verify Postal webhook RSA signature (`X-Postal-Signature-256`, base64 PKCS#1 v1.5 over SHA-256(body)).
    pub fn verify_postal_signature(&self, body: &[u8], signature_b64: &str) -> bool {
        let Some(pem) = &self.postal_public_key_pem else {
            return false;
        };
        verify_postal_rsa(pem, body, signature_b64)
    }

    /// Live Postal needs `SMTP_HOST`, `SMTP_PASS`, and `POSTAL_WEBHOOK_PUBLIC_KEY_FILE`.
    pub fn postal_live_configured(&self) -> bool {
        self.postal_public_key_pem.is_some()
            && self.smtp_host.is_some()
            && self.smtp_password.is_some()
    }
}

fn verify_postal_rsa(pem: &str, body: &[u8], signature_b64: &str) -> bool {
    use base64::Engine;
    use rsa::pkcs1v15::VerifyingKey;
    use rsa::pkcs8::DecodePublicKey;
    use rsa::signature::Verifier;
    use rsa::RsaPublicKey;
    use sha2::Sha256;
    if signature_b64.trim().is_empty() {
        return false;
    }
    let Ok(sig_bytes) = base64::engine::general_purpose::STANDARD.decode(signature_b64.trim())
    else {
        return false;
    };
    let Ok(key) = RsaPublicKey::from_public_key_pem(pem) else {
        return false;
    };
    let verifying_key = VerifyingKey::<Sha256>::new(key);
    let Ok(sig) = rsa::pkcs1v15::Signature::try_from(sig_bytes.as_slice()) else {
        return false;
    };
    verifying_key.verify(body, &sig).is_ok()
}

impl Mailer {
    #[cfg(test)]
    pub fn captured(&self) -> Vec<CapturedMail> {
        self.outbox.inner.lock().unwrap().clone()
    }

    #[cfg(test)]
    pub(crate) fn with_postal_pubkey_pem(mut self, pem: String) -> Self {
        self.postal_public_key_pem = Some(pem);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Postal production RSA public key (parse-only; no secrets).
    const POSTAL_PROD_PUBLIC_KEY_PEM: &str = "-----BEGIN PUBLIC KEY-----
MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQC7bC011pQQFvlLR8gby2vfJWhj
Lv3DKNlKI7q+nRGJyJNHApi1ahe9zJ0dUWbx64jWzOPnAW5WC+cSmsbvyy7qgeXv
0Rv2OtKWqmBZ5q6JXne5h1NWb0J7k7SHTYPyLitVjAOoth1MABuKT5M00g8vkCer
FdfgleEWxIJ9soS53wIDAQAB
-----END PUBLIC KEY-----";

    fn empty_test_config() -> MailConfig {
        MailConfig {
            smtp_host: None,
            smtp_port: 25,
            smtp_password: None,
            smtp_from: "noreply@heavenly.cl".into(),
            public_url: "https://flight.heavenly.cl".into(),
            postal_public_key_pem: None,
        }
    }

    #[test]
    fn smtp_pass_file_trims_newline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("smtp_pass");
        std::fs::write(&path, "from-file-secret\n").unwrap();
        let path_str = path.to_string_lossy().into_owned();
        let pass = read_smtp_password(&mut |key| match key {
            "SMTP_PASS_FILE" => Some(path_str.clone()),
            "SMTP_PASS" => Some("from-env".into()),
            _ => None,
        });
        assert_eq!(pass.as_deref(), Some("from-file-secret"));
    }

    #[test]
    fn smtp_pass_env_when_no_file() {
        let pass = read_smtp_password(&mut |key| {
            if key == "SMTP_PASS" {
                Some("env-only".into())
            } else {
                None
            }
        });
        assert_eq!(pass.as_deref(), Some("env-only"));
    }

    #[test]
    fn send_rate_limit_per_email() {
        let mailer = Mailer::from_config(empty_test_config(), MailOutbox::default());
        for _ in 0..5 {
            assert!(mailer.allow_send("a@test", "1.2.3.4"));
        }
        assert!(!mailer.allow_send("a@test", "1.2.3.4"));
    }

    #[test]
    fn capture_localized_spanish_magic_link() {
        let dir = crate::i18n::Catalogs::resolve_dir(std::path::Path::new("dist"));
        let _ = crate::i18n::init_global(&dir);
        let mailer = Mailer::from_config(empty_test_config(), MailOutbox::default());
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
    fn postal_production_public_key_parses() {
        use rsa::pkcs8::DecodePublicKey;
        use rsa::RsaPublicKey;
        RsaPublicKey::from_public_key_pem(POSTAL_PROD_PUBLIC_KEY_PEM).expect("postal pubkey");
    }

    fn sign_postal_body_sha256(private: &rsa::RsaPrivateKey, body: &[u8]) -> String {
        use base64::Engine;
        use rsa::pkcs1v15::SigningKey;
        use rsa::signature::{SignatureEncoding, Signer};
        use sha2::Sha256;
        let signing = SigningKey::<Sha256>::new(private.clone());
        let signature = signing.sign(body);
        base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
    }

    #[test]
    fn postal_rsa_webhook_signature_roundtrip() {
        use rsa::pkcs8::EncodePublicKey;
        use rsa::RsaPrivateKey;
        let mut rng = rand::thread_rng();
        let private = RsaPrivateKey::new(&mut rng, 2048).expect("rsa key");
        let pem = private
            .to_public_key()
            .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
            .expect("pem");
        let mailer = Mailer::from_config(empty_test_config(), MailOutbox::default())
            .with_postal_pubkey_pem(pem);
        let body = br#"{"to":"x@test"}"#;
        let sig = sign_postal_body_sha256(&private, body);
        assert!(mailer.verify_postal_signature(body, &sig));
        assert!(!mailer.verify_postal_signature(b"tampered", &sig));
        assert!(!mailer.verify_postal_signature(body, "not-valid-base64!!!"));
        assert!(!mailer.verify_postal_signature(body, ""));
    }

    #[test]
    fn postal_verify_rejects_without_configured_key() {
        let mailer = Mailer::from_config(empty_test_config(), MailOutbox::default());
        assert!(!mailer.verify_postal_signature(b"{}", "abc"));
    }

    #[tokio::test]
    async fn smtp_send_failure_does_not_capture_magic_link() {
        let dir = crate::i18n::Catalogs::resolve_dir(std::path::Path::new("dist"));
        let _ = crate::i18n::init_global(&dir);
        let mailer = Mailer::from_config(
            MailConfig {
                smtp_host: Some("127.0.0.1".into()),
                smtp_port: 9,
                smtp_password: None,
                smtp_from: "noreply@heavenly.cl".into(),
                public_url: "https://flight.heavenly.cl".into(),
                postal_public_key_pem: None,
            },
            MailOutbox::default(),
        );
        mailer
            .send_localized(
                "p@test",
                "es",
                "magicLink",
                &[("minutes", "15"), ("url", "https://example.invalid/secret")],
            )
            .await;
        assert!(mailer.captured().is_empty());
    }
}
