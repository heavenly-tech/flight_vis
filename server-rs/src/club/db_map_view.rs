use crate::club::{
    db::{ClubDb, FlightRow},
    dem::DemIndex,
    map::{dedupe_by_tail, DeviceFix, TailLivePosition},
    map_filter::{
        apply_public_cap, public_track_passes, visibility_is_public, AltitudeMode, MapViewFilter,
    },
    visibility::{
        can_view_flight, includes_in_club_map, includes_in_public_map, show_pilot_name,
        FlightVisibility, PilotStatus, Viewer,
    },
};
use rusqlite::{params, OptionalExtension};

impl ClubDb {
    pub fn dem_index(&self) -> DemIndex {
        DemIndex::new(self.data_dir())
    }

    pub fn pilot_is_approved_member(&self, pilot_id: &str, club_id: &str) -> bool {
        self.with_conn(|conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM club_memberships
                 WHERE pilot_id = ?1 AND club_id = ?2 AND status = 'approved'",
                params![pilot_id, club_id],
                |r| r.get(0),
            )?;
            Ok(n > 0)
        })
        .unwrap_or(false)
    }

    pub fn club_map_filter(&self, club_id: &str) -> MapViewFilter {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT COALESCE(latitude, 0), COALESCE(longitude, 0), public_radius_km,
                        show_public_tracks, public_flights_cap, public_altitude_mode, public_altitude_min_ft
                 FROM clubs WHERE id = ?1",
                params![club_id],
                |r| {
                    Ok(MapViewFilter {
                        center_lat: r.get(0)?,
                        center_lon: r.get(1)?,
                        radius_km: r.get(2)?,
                        show_public_tracks: r.get::<_, i64>(3)? != 0,
                        cap: r.get::<_, i64>(4)? as usize,
                        altitude_mode: AltitudeMode::parse(&r.get::<_, String>(5)?),
                        altitude_min_ft: r.get(6)?,
                        club_id: club_id.to_string(),
                    })
                },
            )
        })
        .unwrap_or_else(|_| MapViewFilter {
            center_lat: 0.0,
            center_lon: 0.0,
            radius_km: 100.0,
            show_public_tracks: true,
            cap: 50,
            altitude_mode: AltitudeMode::Agl,
            altitude_min_ft: 300.0,
            club_id: club_id.to_string(),
        })
    }

    pub fn user_map_filter(&self, pilot_id: &str, club_id: &str) -> MapViewFilter {
        let mut base = self.club_map_filter(club_id);
        // User prefs can tighten public radius/cap/altitude; inherit club show_public_tracks unless overridden per-user column.
        if let Ok((radius, show, cap, mode, min_ft, lat, lon, club_lat, club_lon)) =
            self.with_conn(|conn| {
                conn.query_row(
                    "SELECT p.public_radius_km, p.show_public_tracks, p.public_flights_cap,
                        p.public_altitude_mode, p.public_altitude_min_ft,
                        COALESCE(p.last_latitude, c.latitude, 0),
                        COALESCE(p.last_longitude, c.longitude, 0),
                        COALESCE(c.latitude, 0), COALESCE(c.longitude, 0)
                 FROM pilots p
                 JOIN clubs c ON c.id = ?2
                 WHERE p.id = ?1",
                    params![pilot_id, club_id],
                    |r| {
                        Ok((
                            r.get::<_, f64>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, f64>(4)?,
                            r.get::<_, f64>(5)?,
                            r.get::<_, f64>(6)?,
                            r.get::<_, f64>(7)?,
                            r.get::<_, f64>(8)?,
                        ))
                    },
                )
            })
        {
            base.radius_km = radius;
            base.show_public_tracks = show != 0;
            base.cap = cap as usize;
            base.altitude_mode = AltitudeMode::parse(&mode);
            base.altitude_min_ft = min_ft;
            if lat != 0.0 || lon != 0.0 {
                base.center_lat = lat;
                base.center_lon = lon;
            } else if club_lat != 0.0 || club_lon != 0.0 {
                base.center_lat = club_lat;
                base.center_lon = club_lon;
            }
        }
        base
    }

    fn latest_device_fixes_bbox(
        &self,
        min_lat: f64,
        max_lat: f64,
        min_lon: f64,
        max_lon: f64,
    ) -> Vec<DeviceFix> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT f.id, a.tail, f.pilot_id, p.name, f.visibility, a.club_id,
                        fx.t_ms, fx.lat, fx.lon, fx.alt, fx.alt_ref, fx.speed, fx.course, fx.accuracy
                 FROM flights f
                 JOIN aircraft a ON a.id = f.aircraft_id
                 JOIN pilots p ON p.id = f.pilot_id
                 JOIN fixes fx ON fx.flight_id = f.id
                 WHERE f.status = 'active'
                   AND fx.lat BETWEEN ?1 AND ?2
                   AND fx.lon BETWEEN ?3 AND ?4
                   AND fx.t_ms = (
                     SELECT MAX(fx2.t_ms) FROM fixes fx2 WHERE fx2.flight_id = f.id
                   )",
            )?;
            let rows = stmt.query_map(params![min_lat, max_lat, min_lon, max_lon], |r| {
                Ok(DeviceFix {
                    flight_id: r.get(0)?,
                    tail: r.get(1)?,
                    pilot_id: r.get(2)?,
                    pilot_name: r.get(3)?,
                    visibility: r.get(4)?,
                    club_id: r.get(5)?,
                    fix: crate::live::LiveFix {
                        t: r.get(6)?,
                        lat: r.get(7)?,
                        lon: r.get(8)?,
                        alt: r.get(9)?,
                        alt_ref: r.get(10)?,
                        speed: r.get(11)?,
                        course: r.get(12)?,
                        accuracy: r.get(13)?,
                    },
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .unwrap_or_default()
    }

    fn latest_device_fixes_unbounded(&self) -> Vec<DeviceFix> {
        self.latest_device_fixes_bbox(-90.0, 90.0, -180.0, 180.0)
    }

    pub fn latest_map_fixes_global(&self) -> Vec<DeviceFix> {
        self.latest_device_fixes_unbounded()
    }

    fn apply_public_view_policy(
        &self,
        rows: Vec<DeviceFix>,
        viewer: &Viewer,
        viewing_club_id: &str,
        filter: &MapViewFilter,
        dem: &DemIndex,
    ) -> Vec<DeviceFix> {
        rows.into_iter()
            .filter(|r| {
                let vis = FlightVisibility::parse(&r.visibility).unwrap_or(FlightVisibility::Club);
                if self.pilot_is_approved_member(&r.pilot_id, viewing_club_id) {
                    return match viewer {
                        Viewer::Pilot {
                            status: PilotStatus::Approved,
                            ..
                        } => {
                            includes_in_club_map(viewer, &r.pilot_id, vis)
                                && can_view_flight(viewer, &r.pilot_id, vis)
                        }
                        _ => false,
                    };
                }
                if !filter.show_public_tracks || !visibility_is_public(&r.visibility) {
                    return false;
                }
                public_track_passes(r, filter, dem)
            })
            .collect()
    }

    pub fn club_map_for_viewer(&self, viewer: &Viewer) -> Vec<TailLivePosition> {
        let (viewing_club, filter) = match viewer {
            Viewer::Pilot {
                id,
                status: PilotStatus::Approved,
                ..
            } => {
                let club = self.pilot_club_id(id).unwrap_or_default();
                (club.clone(), self.user_map_filter(id, &club))
            }
            Viewer::Anonymous
            | Viewer::Pilot {
                status: PilotStatus::Pending,
                ..
            } => {
                let club = self.default_club_id().unwrap_or_default();
                (club.clone(), self.club_map_filter(&club))
            }
        };
        let dem = self.dem_index();
        let rows = self.latest_map_fixes_global();
        let filtered = self.apply_public_view_policy(rows, viewer, &viewing_club, &filter, &dem);
        let mut positions = dedupe_by_tail(filtered);
        for pos in &mut positions {
            if let Some(f) = self.get_flight(&pos.flight_id) {
                pos.pilot_name = Some(f.pilot_name);
            }
        }
        apply_public_cap(
            positions,
            filter.cap,
            (filter.center_lat, filter.center_lon),
        )
    }

    pub fn public_map(&self) -> Vec<TailLivePosition> {
        let club = self.default_club_id().unwrap_or_default();
        let filter = self.club_map_filter(&club);
        let dem = self.dem_index();
        let rows = self.latest_map_fixes_global();
        let filtered: Vec<DeviceFix> = rows
            .into_iter()
            .filter(|r| {
                if self.pilot_is_approved_member(&r.pilot_id, &club) {
                    return includes_in_public_map(
                        FlightVisibility::parse(&r.visibility).unwrap_or(FlightVisibility::Club),
                    );
                }
                filter.show_public_tracks
                    && visibility_is_public(&r.visibility)
                    && public_track_passes(r, &filter, &dem)
            })
            .collect();
        let mut positions = dedupe_by_tail(filtered);
        for pos in &mut positions {
            let flight = self.get_flight(&pos.flight_id);
            if let Some(f) = flight {
                if show_pilot_name(&Viewer::Anonymous, &f.pilot_id, f.show_pilot_name_public) {
                    pos.pilot_name = Some(f.pilot_name);
                } else {
                    pos.pilot_name = None;
                }
            }
        }
        apply_public_cap(
            positions,
            filter.cap,
            (filter.center_lat, filter.center_lon),
        )
    }

    pub fn flight_visible_to(&self, viewer: &Viewer, flight_id: &str) -> Option<FlightRow> {
        let flight = self.get_flight(flight_id)?;
        let vis = FlightVisibility::parse(&flight.visibility)?;
        if !can_view_flight(viewer, &flight.pilot_id, vis) {
            return None;
        }
        let club_id = self
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT a.club_id FROM flights f JOIN aircraft a ON a.id = f.aircraft_id WHERE f.id = ?1",
                    params![flight_id],
                    |r| r.get::<_, String>(0),
                )
            })
            .ok()?;
        let viewing_club = match viewer {
            Viewer::Pilot {
                id,
                status: PilotStatus::Approved,
                ..
            } => self.pilot_club_id(id).unwrap_or_else(|_| club_id.clone()),
            Viewer::Anonymous | Viewer::Pilot { .. } => {
                self.default_club_id().unwrap_or_else(|_| club_id.clone())
            }
        };
        if self.pilot_is_approved_member(&flight.pilot_id, &viewing_club) {
            return Some(flight);
        }
        if vis != FlightVisibility::Public {
            return None;
        }
        let filter = match viewer {
            Viewer::Pilot {
                id,
                status: PilotStatus::Approved,
                ..
            } => self.user_map_filter(id, &club_id),
            Viewer::Pilot { .. } | Viewer::Anonymous => self.club_map_filter(&club_id),
        };
        if !filter.show_public_tracks {
            return None;
        }
        let dem = self.dem_index();
        let latest: Option<DeviceFix> = self
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT a.tail, f.pilot_id, p.name, f.visibility, a.club_id,
                            fx.t_ms, fx.lat, fx.lon, fx.alt, fx.alt_ref, fx.speed, fx.course, fx.accuracy
                     FROM flights f
                     JOIN aircraft a ON a.id = f.aircraft_id
                     JOIN pilots p ON p.id = f.pilot_id
                     JOIN fixes fx ON fx.flight_id = f.id
                     WHERE f.id = ?1
                     ORDER BY fx.t_ms DESC LIMIT 1",
                    params![flight_id],
                    |r| {
                        Ok(DeviceFix {
                            flight_id: flight_id.to_string(),
                            tail: r.get(0)?,
                            pilot_id: r.get(1)?,
                            pilot_name: r.get(2)?,
                            visibility: r.get(3)?,
                            club_id: r.get(4)?,
                            fix: crate::live::LiveFix {
                                t: r.get(5)?,
                                lat: r.get(6)?,
                                lon: r.get(7)?,
                                alt: r.get(8)?,
                                alt_ref: r.get(9)?,
                                speed: r.get(10)?,
                                course: r.get(11)?,
                                accuracy: r.get(12)?,
                            },
                        })
                    },
                )
                .optional()
            })
            .ok()
            .flatten();
        let row = latest?;
        if public_track_passes(&row, &filter, &dem) {
            Some(flight)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::club::{
        auth::new_id,
        db::{now_ms, ClubDb},
        visibility::PilotRole,
        visibility::{FlightVisibility, Viewer},
    };
    use crate::live::LiveFix;

    fn second_club(db: &ClubDb, slug: &str) -> String {
        let id = new_id();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO clubs (id, name, slug, created_at, latitude, longitude, show_public_tracks)
                 VALUES (?1, ?2, ?3, ?4, -33.0, -70.0, 1)",
                params![id, "Club B", slug, now_ms()],
            )
        })
        .unwrap();
        id
    }

    #[test]
    fn club_show_public_off_hides_other_club_public() {
        let db = ClubDb::open_in_memory().unwrap();
        let club_a = db.default_club_id().unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE clubs SET latitude = -33.0, longitude = -70.0, show_public_tracks = 0 WHERE id = ?1",
                params![club_a],
            )
        })
        .unwrap();
        let club_b = second_club(&db, "club-b");
        db.upsert_type(&club_b, "C152").unwrap();
        db.upsert_aircraft(&club_b, "N-OUT", "C152", true).unwrap();
        let outsider = db
            .register(&club_b, "Out", "out@test", "", "password1234")
            .unwrap();
        db.approve_pilot(&outsider).unwrap();
        db.add_checkout(&outsider, "C152", None, None).unwrap();
        let dev = db.create_device(&outsider, "d").unwrap().0;
        db.issue_open_checkin(&outsider, &dev, &club_b, now_ms())
            .unwrap();
        let fid = db
            .start_flight(
                &outsider,
                &dev,
                "N-OUT",
                FlightVisibility::Public,
                false,
                now_ms(),
            )
            .unwrap();
        db.append_fix(
            &fid,
            &dev,
            &LiveFix {
                t: now_ms(),
                lat: -33.01,
                lon: -70.0,
                alt: Some(1500.0),
                alt_ref: Some("hae".into()),
                speed: None,
                course: None,
                accuracy: None,
            },
        )
        .unwrap();
        assert!(db.public_map().is_empty());
    }

    #[test]
    fn member_club_flight_unaffected_by_show_public_off() {
        let db = ClubDb::open_in_memory().unwrap();
        let club = db.default_club_id().unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE clubs SET latitude = -33.0, longitude = -70.0, show_public_tracks = 0 WHERE id = ?1",
                params![club],
            )
        })
        .unwrap();
        db.upsert_type(&club, "C152").unwrap();
        db.upsert_aircraft(&club, "N-M", "C152", true).unwrap();
        let member = db
            .register(&club, "Mem", "mem@test", "", "password1234")
            .unwrap();
        db.approve_pilot(&member).unwrap();
        db.add_checkout(&member, "C152", None, None).unwrap();
        let dev = db.create_device(&member, "d").unwrap().0;
        db.issue_open_checkin(&member, &dev, &club, now_ms())
            .unwrap();
        let fid = db
            .start_flight(
                &member,
                &dev,
                "N-M",
                FlightVisibility::Club,
                false,
                now_ms(),
            )
            .unwrap();
        db.append_fix(
            &fid,
            &dev,
            &LiveFix {
                t: now_ms(),
                lat: -33.01,
                lon: -70.0,
                alt: Some(1500.0),
                alt_ref: None,
                speed: None,
                course: None,
                accuracy: None,
            },
        )
        .unwrap();
        let viewer = Viewer::Pilot {
            id: member,
            status: PilotStatus::Approved,
            role: PilotRole::Pilot,
        };
        assert_eq!(db.club_map_for_viewer(&viewer).len(), 1);
    }
}
