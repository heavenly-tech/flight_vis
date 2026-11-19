//! Who may see a flight, fixes, and pilot names on each surface.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightVisibility {
    Private,
    Club,
    Public,
}

impl FlightVisibility {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "private" => Some(Self::Private),
            "club" => Some(Self::Club),
            "public" => Some(Self::Public),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Club => "club",
            Self::Public => "public",
        }
    }

    pub fn default_start() -> Self {
        Self::Club
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PilotStatus {
    Pending,
    Approved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PilotRole {
    Pilot,
    Admin,
}

#[derive(Clone, Debug)]
pub enum Viewer {
    Anonymous,
    Pilot {
        id: String,
        status: PilotStatus,
        role: PilotRole,
    },
}

/// Whether this viewer may read fixes/history/stream for the flight.
pub fn can_view_flight(
    viewer: &Viewer,
    owner_pilot_id: &str,
    visibility: FlightVisibility,
) -> bool {
    match viewer {
        Viewer::Anonymous => visibility == FlightVisibility::Public,
        Viewer::Pilot {
            id,
            status: PilotStatus::Pending,
            ..
        } => id == owner_pilot_id,
        Viewer::Pilot {
            status: PilotStatus::Approved,
            role: PilotRole::Admin,
            ..
        } => true,
        Viewer::Pilot {
            id,
            status: PilotStatus::Approved,
            role: PilotRole::Pilot,
            ..
        } => {
            if id == owner_pilot_id {
                return true;
            }
            matches!(
                visibility,
                FlightVisibility::Club | FlightVisibility::Public
            )
        }
    }
}

/// Whether the viewer may see the pilot's name in the payload.
pub fn show_pilot_name(
    viewer: &Viewer,
    owner_pilot_id: &str,
    show_pilot_name_public: bool,
) -> bool {
    match viewer {
        Viewer::Anonymous => show_pilot_name_public,
        Viewer::Pilot {
            id,
            status: PilotStatus::Pending,
            ..
        } => id == owner_pilot_id,
        Viewer::Pilot {
            status: PilotStatus::Approved,
            role: PilotRole::Admin,
            ..
        } => true,
        Viewer::Pilot {
            id,
            status: PilotStatus::Approved,
            role: PilotRole::Pilot,
            ..
        } => id == owner_pilot_id || show_pilot_name_public,
    }
}

/// Approved-only club map surface (not the anonymous public map).
pub fn includes_in_club_map(viewer: &Viewer, owner: &str, visibility: FlightVisibility) -> bool {
    match viewer {
        Viewer::Pilot {
            status: PilotStatus::Approved,
            ..
        } => can_view_flight(viewer, owner, visibility),
        _ => false,
    }
}

/// Anonymous public map: public flights only.
pub fn includes_in_public_map(visibility: FlightVisibility) -> bool {
    visibility == FlightVisibility::Public
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approved_pilot(id: &str) -> Viewer {
        Viewer::Pilot {
            id: id.to_string(),
            status: PilotStatus::Approved,
            role: PilotRole::Pilot,
        }
    }

    fn pending_pilot(id: &str) -> Viewer {
        Viewer::Pilot {
            id: id.to_string(),
            status: PilotStatus::Pending,
            role: PilotRole::Pilot,
        }
    }

    fn admin(id: &str) -> Viewer {
        Viewer::Pilot {
            id: id.to_string(),
            status: PilotStatus::Approved,
            role: PilotRole::Admin,
        }
    }

    #[test]
    fn private_hidden_from_other_pilots() {
        let v = approved_pilot("b");
        assert!(!can_view_flight(&v, "a", FlightVisibility::Private));
    }

    #[test]
    fn private_visible_to_owner_and_admin() {
        assert!(can_view_flight(
            &approved_pilot("a"),
            "a",
            FlightVisibility::Private
        ));
        assert!(can_view_flight(&admin("x"), "a", FlightVisibility::Private));
    }

    #[test]
    fn club_visible_to_approved_not_anonymous() {
        assert!(can_view_flight(
            &approved_pilot("b"),
            "a",
            FlightVisibility::Club
        ));
        assert!(!can_view_flight(
            &Viewer::Anonymous,
            "a",
            FlightVisibility::Club
        ));
    }

    #[test]
    fn public_visible_anonymous() {
        assert!(can_view_flight(
            &Viewer::Anonymous,
            "a",
            FlightVisibility::Public
        ));
    }

    #[test]
    fn pending_sees_nothing_except_own() {
        let v = pending_pilot("p");
        assert!(!can_view_flight(&v, "a", FlightVisibility::Public));
        assert!(can_view_flight(&v, "p", FlightVisibility::Private));
        assert!(!includes_in_club_map(&v, "p", FlightVisibility::Club));
    }

    #[test]
    fn pilot_name_anonymous_requires_flag() {
        assert!(!show_pilot_name(&Viewer::Anonymous, "a", false));
        assert!(show_pilot_name(&Viewer::Anonymous, "a", true));
    }

    #[test]
    fn public_map_only_public() {
        assert!(includes_in_public_map(FlightVisibility::Public));
        assert!(!includes_in_public_map(FlightVisibility::Club));
    }
}
