//! Normalized club membership role/status (single authority in `club_memberships`).

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MembershipStatus {
    Pending,
    Active,
    Suspended,
}

impl MembershipStatus {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "active" | "approved" => Self::Active,
            "suspended" => Self::Suspended,
            _ => Self::Pending,
        }
    }

    #[allow(dead_code)]
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Suspended => "suspended",
        }
    }

    pub fn allows_club_api(self) -> bool {
        matches!(self, Self::Active)
    }
}

pub fn membership_role_is_admin(role: &str) -> bool {
    role == "admin"
}

#[allow(dead_code)]
pub fn membership_role_is_staff_or_admin(role: &str) -> bool {
    role == "admin" || role == "staff"
}
