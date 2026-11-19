use serde::Serialize;

/// Chile club tail format: `CC-` plus exactly three alphanumeric characters (uppercased).
pub fn normalize_tail(input: &str) -> Result<String, &'static str> {
    let t = input.trim().to_uppercase();
    let ok = t.len() == 6
        && t.starts_with("CC-")
        && t.as_bytes()[3..].iter().all(|b| b.is_ascii_alphanumeric());
    if ok {
        Ok(t)
    } else {
        Err("tail must match CC-XXX (three alphanumeric characters)")
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessRequestView {
    pub id: String,
    pub requested_tail: String,
    pub type_code: Option<String>,
    pub new_type_name: Option<String>,
    pub status: String,
    pub reject_reason: Option<String>,
    pub created_at: i64,
    pub decided_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminAccessRequestView {
    #[serde(flatten)]
    pub base: AccessRequestView,
    pub pilot_id: String,
    pub pilot_name: String,
    pub pilot_email: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessRequestError {
    InvalidTail,
    MissingType,
    NoCheckin,
    DuplicatePending,
    NotFound,
    NotPending,
    TypeMismatch,
    Forbidden,
    Db,
}

impl From<sqlx::Error> for AccessRequestError {
    fn from(_: sqlx::Error) -> Self {
        Self::Db
    }
}

impl AccessRequestError {
    pub fn message(self) -> &'static str {
        match self {
            Self::InvalidTail => "invalid tail format",
            Self::MissingType => "type id or new type name required",
            Self::NoCheckin => "check-in required",
            Self::DuplicatePending => "pending request already exists for this tail",
            Self::NotFound => "not found",
            Self::NotPending => "not pending",
            Self::TypeMismatch => "type does not match existing tail",
            Self::Forbidden => "forbidden",
            Self::Db => "db error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_format() {
        assert_eq!(normalize_tail("cc-abc").unwrap(), "CC-ABC");
        assert!(normalize_tail("N123").is_err());
        assert!(normalize_tail("CC-AB").is_err());
    }
}
