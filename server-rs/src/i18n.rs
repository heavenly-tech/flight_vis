//! Locale catalogs (`locales/*.json`) loaded at startup; no per-string code changes when adding a language.

use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone)]
pub struct Catalogs {
    pub canonical: String,
    locales: BTreeMap<String, LocaleMeta>,
    messages: HashMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LocaleMeta {
    pub label: String,
    pub bcp47: String,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    canonical: String,
    locales: BTreeMap<String, LocaleMeta>,
}

#[derive(Debug)]
pub enum I18nError {
    MissingManifest,
    MissingCatalog(String),
    KeyMismatch {
        locale: String,
        missing: Vec<String>,
        extra: Vec<String>,
    },
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for I18nError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingManifest => write!(f, "locales/manifest.json missing"),
            Self::MissingCatalog(l) => write!(f, "locale catalog missing: {l}"),
            Self::KeyMismatch {
                locale,
                missing,
                extra,
            } => write!(
                f,
                "locale {locale}: missing {} keys, extra {}",
                missing.len(),
                extra.len()
            ),
            Self::Io(e) => write!(f, "{e}"),
            Self::Json(e) => write!(f, "{e}"),
        }
    }
}

impl From<std::io::Error> for I18nError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for I18nError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

fn flatten_value(prefix: &str, value: &serde_json::Value, out: &mut BTreeMap<String, String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if k == "_meta" {
                    continue;
                }
                let p = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_value(&p, v, out);
            }
        }
        serde_json::Value::String(s) => {
            out.insert(prefix.to_string(), s.clone());
        }
        _ => {}
    }
}

fn flatten_catalog(json: &serde_json::Value) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    flatten_value("", json, &mut out);
    out
}

impl Catalogs {
    pub fn load_from_dir(dir: &Path) -> Result<Self, I18nError> {
        let manifest_path = dir.join("manifest.json");
        if !manifest_path.is_file() {
            return Err(I18nError::MissingManifest);
        }
        let manifest: Manifest = serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
        let canonical = manifest.canonical.clone();
        let canon_path = dir.join(format!("{canonical}.json"));
        if !canon_path.is_file() {
            return Err(I18nError::MissingCatalog(canonical));
        }
        let canon_json: serde_json::Value = serde_json::from_slice(&std::fs::read(&canon_path)?)?;
        let canon_keys: BTreeSet<String> = flatten_catalog(&canon_json).into_keys().collect();
        let mut messages = HashMap::new();
        messages.insert(canonical.clone(), flatten_catalog(&canon_json));
        for id in manifest.locales.keys() {
            if id == &canonical {
                continue;
            }
            let path = dir.join(format!("{id}.json"));
            if !path.is_file() {
                return Err(I18nError::MissingCatalog(id.clone()));
            }
            let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            let flat = flatten_catalog(&json);
            let keys: BTreeSet<_> = flat.keys().cloned().collect();
            let missing: Vec<_> = canon_keys.difference(&keys).cloned().collect();
            let extra: Vec<_> = keys.difference(&canon_keys).cloned().collect();
            if !missing.is_empty() || !extra.is_empty() {
                return Err(I18nError::KeyMismatch {
                    locale: id.clone(),
                    missing,
                    extra,
                });
            }
            messages.insert(id.clone(), flat);
        }
        Ok(Self {
            canonical,
            locales: manifest.locales,
            messages,
        })
    }

    pub fn resolve_dir(dist: &Path) -> PathBuf {
        if let Ok(dir) = std::env::var("FLIGHT_I18N_LOCALES_DIR") {
            let path = PathBuf::from(&dir);
            if path.join("manifest.json").is_file() {
                return path;
            }
        }
        let in_dist = dist.join("locales");
        if in_dist.join("manifest.json").is_file() {
            return in_dist;
        }
        // Legacy vite-plugin-static-copy nested `locales/` under `dist/locales/`.
        let nested = in_dist.join("locales");
        if nested.join("manifest.json").is_file() {
            return nested;
        }
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../locales");
        if repo.join("manifest.json").is_file() {
            return repo;
        }
        PathBuf::from("locales")
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.locales.keys().map(String::as_str)
    }

    pub fn locale_labels(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        self.ids().filter_map(|id| {
            self.locales
                .get(id)
                .map(|meta| (id, meta.label.as_str(), meta.bcp47.as_str()))
        })
    }

    pub fn is_known(&self, locale: &str) -> bool {
        self.messages.contains_key(locale)
    }

    pub fn normalize(locale: &str) -> String {
        locale
            .split('-')
            .next()
            .unwrap_or(locale)
            .to_ascii_lowercase()
    }

    pub fn resolve_locale(
        &self,
        user: Option<&str>,
        club_default: Option<&str>,
        accept_language: Option<&str>,
        stored: Option<&str>,
    ) -> String {
        let pick = |v: Option<&str>| {
            v.map(Self::normalize)
                .filter(|id| self.is_known(id))
                .map(|id| id.to_string())
        };
        pick(stored)
            .or_else(|| pick(user))
            .or_else(|| {
                accept_language
                    .and_then(parse_accept)
                    .and_then(|id| pick(Some(id.as_str())))
            })
            .or_else(|| pick(club_default))
            .unwrap_or_else(|| self.canonical.clone())
    }

    pub fn t(&self, locale: &str, key: &str, vars: &[(&str, &str)]) -> String {
        let id = if self.is_known(locale) {
            locale
        } else {
            self.canonical.as_str()
        };
        let table = self
            .messages
            .get(id)
            .or_else(|| self.messages.get(&self.canonical));
        let Some(table) = table else {
            return key.to_string();
        };
        let mut text = table.get(key).cloned().unwrap_or_else(|| key.to_string());
        for (k, v) in vars {
            text = text.replace(&format!("{{{k}}}"), v);
        }
        text
    }

    pub fn mail(
        &self,
        locale: &str,
        stem: &str,
        vars: &[(&str, &str)],
    ) -> (String, String, String) {
        let subj = self.t(locale, &format!("mail.{stem}.subject"), vars);
        let plain = self.t(locale, &format!("mail.{stem}.bodyPlain"), vars);
        let html = self.t(locale, &format!("mail.{stem}.bodyHtml"), vars);
        (subj, plain, html)
    }
}

#[allow(dead_code)]
fn parse_accept(header: &str) -> Option<String> {
    let first = header.split(',').next()?.trim().to_ascii_lowercase();
    let id = first.split('-').next()?.to_string();
    Some(id)
}

static GLOBAL: std::sync::OnceLock<Arc<Catalogs>> = std::sync::OnceLock::new();

pub fn init_global(dir: &Path) -> Result<Arc<Catalogs>, I18nError> {
    let catalogs = Arc::new(Catalogs::load_from_dir(dir)?);
    let _ = GLOBAL.set(catalogs.clone());
    Ok(catalogs)
}

pub fn global() -> Option<Arc<Catalogs>> {
    GLOBAL.get().cloned()
}

pub fn require_global() -> Arc<Catalogs> {
    global().expect("i18n not initialized")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_repo_locales() {
        let dir = Catalogs::resolve_dir(Path::new("dist"));
        let cat = Catalogs::load_from_dir(&dir).expect("catalogs");
        assert!(cat.is_known("es"));
        assert!(cat.is_known("en"));
        let es = cat.t("es", "club.login.title", &[]);
        assert!(!es.is_empty());
    }

    #[test]
    fn resolve_prefers_user() {
        let dir = Catalogs::resolve_dir(Path::new("dist"));
        let cat = Catalogs::load_from_dir(&dir).unwrap();
        assert_eq!(cat.resolve_locale(Some("en"), Some("es"), None, None), "en");
    }

    #[test]
    fn resolve_defaults_spanish_without_user_or_accept() {
        let dir = Catalogs::resolve_dir(Path::new("dist"));
        let cat = Catalogs::load_from_dir(&dir).unwrap();
        assert_eq!(cat.resolve_locale(None, None, Some("en"), None), "en");
        assert_eq!(cat.resolve_locale(None, None, Some("en"), Some("es")), "es");
    }
}
