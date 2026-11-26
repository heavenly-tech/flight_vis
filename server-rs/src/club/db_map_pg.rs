//! Club-scoped map views and flight visibility (Postgres).

use crate::club::{
    db::{now_ms, ClubDb, DbError, FlightRow},
    dem::DemIndex,
    geo::{bbox_for_radius_km, haversine_km},
    map::{dedupe_by_tail, filter_admin_tails, DeviceFix, TailLivePosition},
    map_filter::{
        apply_public_cap, evaluate_public_altitude, public_track_passes, visibility_is_public,
        AltitudeEval, AltitudeMode, MapViewFilter,
    },
    trail::{
        TrailMinutes, DEFAULT_NEARBY_PUBLIC_RADIUS_KM, DEFAULT_TRAIL_MINUTES, LIVE_FIX_MAX_AGE_MS,
        MAX_NEARBY_PUBLIC_FLIGHTS,
    },
    visibility::{
        can_view_flight, includes_in_club_map, includes_in_public_map, show_pilot_name,
        show_pilot_name_admin_map, FlightVisibility, PilotRole, PilotStatus, Viewer,
    },
};
use sqlx::Row;
use std::path::PathBuf;

fn club_data_dir() -> PathBuf {
    std::env::var("FLIGHT_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("data"))
}

impl ClubDb {
    pub fn dem_index(&self) -> DemIndex {
        DemIndex::new(&club_data_dir())
    }

    pub async fn pilot_is_approved_member(&self, pilot_id: &str, club_id: &str) -> bool {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM club_memberships
             WHERE pilot_id = $1 AND club_id = $2 AND status IN ('active', 'approved')",
        )
        .bind(pilot_id)
        .bind(club_id)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        n > 0
    }

    pub async fn flight_aircraft_club_id(&self, flight_id: &str) -> Option<String> {
        sqlx::query_scalar(
            "SELECT a.club_id FROM flights f JOIN aircraft a ON a.id = f.aircraft_id WHERE f.id = $1",
        )
        .bind(flight_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    pub async fn club_map_filter(&self, club_id: &str) -> MapViewFilter {
        let row = sqlx::query(
            "SELECT COALESCE(latitude, 0), COALESCE(longitude, 0), public_radius_km,
                    show_public_tracks, public_flights_cap, public_altitude_mode, public_altitude_min_ft
             FROM clubs WHERE id = $1",
        )
        .bind(club_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();

        row.map(|r| MapViewFilter {
            center_lat: r.try_get(0).unwrap_or(0.0),
            center_lon: r.try_get(1).unwrap_or(0.0),
            radius_km: r.try_get(2).unwrap_or(100.0),
            show_public_tracks: r.try_get::<i64, _>(3).unwrap_or(1) != 0,
            cap: r.try_get::<i64, _>(4).unwrap_or(50) as usize,
            altitude_mode: AltitudeMode::parse(
                &r.try_get::<String, _>(5)
                    .unwrap_or_else(|_| "agl".to_string()),
            ),
            altitude_min_ft: r.try_get(6).unwrap_or(300.0),
            club_id: club_id.to_string(),
        })
        .unwrap_or_else(|| MapViewFilter {
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

    pub async fn user_map_filter(&self, pilot_id: &str, club_id: &str) -> MapViewFilter {
        let mut base = self.club_map_filter(club_id).await;
        let row = sqlx::query(
            "SELECT p.public_radius_km, p.show_public_tracks, p.public_flights_cap,
                    p.public_altitude_mode, p.public_altitude_min_ft,
                    COALESCE(p.last_latitude, c.latitude, 0),
                    COALESCE(p.last_longitude, c.longitude, 0),
                    COALESCE(c.latitude, 0), COALESCE(c.longitude, 0)
             FROM pilots p
             JOIN clubs c ON c.id = $2
             WHERE p.id = $1",
        )
        .bind(pilot_id)
        .bind(club_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();

        if let Some(r) = row {
            base.radius_km = r.try_get(0).unwrap_or(base.radius_km);
            base.show_public_tracks = r.try_get::<i64, _>(1).unwrap_or(1) != 0;
            base.cap = r.try_get::<i64, _>(2).unwrap_or(base.cap as i64) as usize;
            base.altitude_mode = AltitudeMode::parse(
                &r.try_get::<String, _>(3)
                    .unwrap_or_else(|_| "agl".to_string()),
            );
            base.altitude_min_ft = r.try_get(4).unwrap_or(base.altitude_min_ft);
            let lat: f64 = r.try_get(5).unwrap_or(0.0);
            let lon: f64 = r.try_get(6).unwrap_or(0.0);
            let club_lat: f64 = r.try_get(7).unwrap_or(0.0);
            let club_lon: f64 = r.try_get(8).unwrap_or(0.0);
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

    async fn apply_public_view_policy(
        &self,
        rows: Vec<DeviceFix>,
        viewer: &Viewer,
        viewing_club_id: &str,
        filter: &MapViewFilter,
        dem: &DemIndex,
    ) -> Vec<DeviceFix> {
        let mut out = Vec::new();
        for r in rows {
            let vis = FlightVisibility::parse(&r.visibility).unwrap_or(FlightVisibility::Club);
            if self
                .pilot_is_approved_member(&r.pilot_id, viewing_club_id)
                .await
            {
                let keep = match viewer {
                    Viewer::Pilot {
                        status: PilotStatus::Approved,
                        ..
                    } => {
                        if r.club_id != viewing_club_id && vis != FlightVisibility::Public {
                            false
                        } else {
                            includes_in_club_map(viewer, &r.pilot_id, vis)
                                && can_view_flight(viewer, &r.pilot_id, vis)
                        }
                    }
                    _ => false,
                };
                if keep {
                    out.push(r);
                }
                continue;
            }
            if !filter.show_public_tracks || !visibility_is_public(&r.visibility) {
                continue;
            }
            if public_track_passes(&r, filter, dem) {
                out.push(r);
            }
        }
        out
    }

    #[allow(dead_code)]
    pub async fn viewing_club_id(&self, viewer: &Viewer) -> Result<String, DbError> {
        match viewer {
            Viewer::Pilot {
                id,
                status: PilotStatus::Approved,
                ..
            } => self.pilot_club_id(id).await,
            Viewer::Anonymous
            | Viewer::Pilot {
                status: PilotStatus::Pending,
                ..
            } => self.default_club_id().await,
        }
    }

    pub async fn flight_visible_to_scoped(
        &self,
        viewer: &Viewer,
        flight_id: &str,
    ) -> Option<FlightRow> {
        let flight = self.get_flight(flight_id).await?;
        let vis = FlightVisibility::parse(&flight.visibility)?;
        if !can_view_flight(viewer, &flight.pilot_id, vis) {
            return None;
        }
        let club_id = self.flight_aircraft_club_id(flight_id).await?;
        let viewing_club = match viewer {
            Viewer::Pilot {
                id,
                status: PilotStatus::Approved,
                ..
            } => self.pilot_club_id(id).await.unwrap_or(club_id.clone()),
            Viewer::Anonymous | Viewer::Pilot { .. } => {
                self.default_club_id().await.unwrap_or(club_id.clone())
            }
        };

        if self
            .pilot_is_approved_member(&flight.pilot_id, &viewing_club)
            .await
        {
            if club_id != viewing_club && vis != FlightVisibility::Public {
                return None;
            }
            if vis == FlightVisibility::Private {
                let viewer_id = match viewer {
                    Viewer::Pilot { id, .. } => id.as_str(),
                    _ => "",
                };
                if flight.pilot_id != viewer_id
                    && self.require_club_admin(viewer_id, &club_id).await.is_err()
                {
                    return None;
                }
            }
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
            } => self.user_map_filter(id, &club_id).await,
            Viewer::Pilot { .. } | Viewer::Anonymous => self.club_map_filter(&club_id).await,
        };
        if !filter.show_public_tracks {
            return None;
        }
        let dem = self.dem_index();
        let row: Option<DeviceFix> = sqlx::query(
            "SELECT f.id, a.tail, f.pilot_id, p.name, f.visibility, a.club_id,
                    fx.t_ms, fx.lat, fx.lon, fx.alt, fx.alt_ref, fx.speed, fx.course, fx.accuracy
             FROM flights f
             JOIN aircraft a ON a.id = f.aircraft_id
             JOIN pilots p ON p.id = f.pilot_id
             JOIN fixes fx ON fx.flight_id = f.id
             WHERE f.id = $1
             ORDER BY fx.t_ms DESC LIMIT 1",
        )
        .bind(flight_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .and_then(|r| {
            Some(DeviceFix {
                flight_id: flight_id.to_string(),
                tail: r.try_get(1).ok()?,
                pilot_id: r.try_get(2).ok()?,
                pilot_name: r.try_get(3).ok()?,
                visibility: r.try_get(4).ok()?,
                club_id: r.try_get(5).ok()?,
                trail_minutes: crate::club::trail::DEFAULT_TRAIL_MINUTES,
                fix: crate::live::LiveFix {
                    t: r.try_get(6).ok()?,
                    lat: r.try_get(7).ok()?,
                    lon: r.try_get(8).ok()?,
                    alt: r.try_get(9).ok()?,
                    alt_ref: r.try_get(10).ok(),
                    speed: r.try_get(11).ok()?,
                    course: r.try_get(12).ok()?,
                    accuracy: r.try_get(13).ok()?,
                },
            })
        });
        let row = row?;
        if public_track_passes(&row, &filter, &dem) {
            Some(flight)
        } else {
            None
        }
    }

    pub async fn club_map_for_viewer_scoped(&self, viewer: &Viewer) -> Vec<TailLivePosition> {
        let (viewing_club, filter) = match viewer {
            Viewer::Pilot {
                id,
                status: PilotStatus::Approved,
                ..
            } => {
                let club = self.pilot_club_id(id).await.unwrap_or_default();
                let f = self.user_map_filter(id, &club).await;
                (club, f)
            }
            Viewer::Anonymous
            | Viewer::Pilot {
                status: PilotStatus::Pending,
                ..
            } => {
                let club = self.default_club_id().await.unwrap_or_default();
                let f = self.club_map_filter(&club).await;
                (club, f)
            }
        };
        let dem = self.dem_index();
        let rows = self.latest_device_fixes().await;
        let filtered = self
            .apply_public_view_policy(rows, viewer, &viewing_club, &filter, &dem)
            .await;
        let mut positions = dedupe_by_tail(filtered);
        for pos in &mut positions {
            if let Some(f) = self.get_flight(&pos.flight_id).await {
                let home = self
                    .pilot_is_approved_member(&f.pilot_id, &viewing_club)
                    .await;
                let show = if matches!(
                    viewer,
                    Viewer::Pilot {
                        role: PilotRole::Admin,
                        ..
                    }
                ) {
                    show_pilot_name_admin_map(viewer, home, &f.pilot_id, f.show_pilot_name_public)
                } else {
                    show_pilot_name(viewer, &f.pilot_id, f.show_pilot_name_public)
                };
                if show {
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

    pub async fn public_map_scoped(&self, club_id: &str) -> Vec<TailLivePosition> {
        let club = club_id.to_string();
        let filter = self.club_map_filter(&club).await;
        let dem = self.dem_index();
        let rows = self.latest_device_fixes().await;
        let filtered: Vec<DeviceFix> = {
            let mut out = Vec::new();
            for r in rows {
                if r.club_id != club {
                    continue;
                }
                let vis = FlightVisibility::parse(&r.visibility).unwrap_or(FlightVisibility::Club);
                if self.pilot_is_approved_member(&r.pilot_id, &club).await {
                    if includes_in_public_map(vis) {
                        out.push(r);
                    }
                    continue;
                }
                if filter.show_public_tracks
                    && visibility_is_public(&r.visibility)
                    && public_track_passes(&r, &filter, &dem)
                {
                    out.push(r);
                }
            }
            out
        };
        let mut positions = dedupe_by_tail(filtered);
        for pos in &mut positions {
            if let Some(f) = self.get_flight(&pos.flight_id).await {
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

    /// Returns live `public` flights from OTHER active clubs within the viewing club's configured
    /// radius of its base aerodrome, strictly when `clubs.show_nearby_public` is `true` and the
    /// club has valid base coordinates.
    ///
    /// - Never uses `default_club_id`.
    /// - Never returns `private` or other clubs' `club`-only flights.
    /// - Excludes suspended, pending/unverified, and deleted clubs.
    /// - Respects `show_pilot_name_public`; never exposes `pilot_id`.
    /// - Applies the viewing club's public altitude filter (`public_altitude_mode` /
    ///   `public_altitude_min_ft`) via `evaluate_public_altitude` before the nearest-30 cap;
    ///   does not apply legacy `show_public_tracks`, `public_radius_km`, or `public_flights_cap`.
    /// - Capped at `MAX_NEARBY_PUBLIC_FLIGHTS` (30), nearest first.
    /// - Structured via a `candidate_public_flights` CTE so future club-less public flights
    ///   (`f.club_id IS NULL`) can be added by relaxing one predicate.
    pub async fn nearby_public_positions_for_club(
        &self,
        own_club_id: &str,
        now: i64,
    ) -> Vec<TailLivePosition> {
        let club_row = sqlx::query(
            "SELECT show_nearby_public, nearby_public_radius_km, latitude, longitude,
                    public_altitude_mode, public_altitude_min_ft
             FROM clubs
             WHERE id = $1",
        )
        .bind(own_club_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();

        let Some(club_row) = club_row else {
            return Vec::new();
        };
        let show_nearby: bool = club_row.try_get(0).unwrap_or(false);
        if !show_nearby {
            return Vec::new();
        }
        let radius_km: f64 = club_row
            .try_get::<i16, _>(1)
            .unwrap_or(DEFAULT_NEARBY_PUBLIC_RADIUS_KM) as f64;
        let base_lat: Option<f64> = club_row.try_get(2).ok().flatten();
        let base_lon: Option<f64> = club_row.try_get(3).ok().flatten();
        let (Some(base_lat), Some(base_lon)) = (base_lat, base_lon) else {
            return Vec::new();
        };
        if !base_lat.is_finite()
            || !base_lon.is_finite()
            || !(-90.0..=90.0).contains(&base_lat)
            || !(-180.0..=180.0).contains(&base_lon)
        {
            return Vec::new();
        }
        let altitude_mode = AltitudeMode::parse(
            &club_row
                .try_get::<String, _>(4)
                .unwrap_or_else(|_| "agl".to_string()),
        );
        let altitude_min_ft: f64 = club_row.try_get(5).unwrap_or(300.0);

        let (raw_min_lat, raw_max_lat, raw_min_lon, raw_max_lon) =
            bbox_for_radius_km(base_lat, base_lon, radius_km);
        let min_lat = raw_min_lat.max(-90.0);
        let max_lat = raw_max_lat.min(90.0);
        let (min_lon, max_lon) = if !raw_min_lon.is_finite()
            || !raw_max_lon.is_finite()
            || raw_min_lon < -180.0
            || raw_max_lon > 180.0
        {
            (-180.0, 180.0)
        } else {
            (raw_min_lon, raw_max_lon)
        };

        let live_cutoff_ms = now.saturating_sub(LIVE_FIX_MAX_AGE_MS);

        let rows = sqlx::query(
            "WITH candidate_public_flights AS (
                 SELECT f.id AS flight_id,
                        f.club_id,
                        f.show_pilot_name_public,
                        f.trail_minutes,
                        c.trail_minutes_default,
                        a.tail,
                        p.name AS pilot_name,
                        fx.t_ms,
                        fx.lat,
                        fx.lon,
                        fx.alt,
                        fx.alt_ref,
                        fx.speed,
                        fx.course,
                        fx.accuracy
                 FROM flights f
                 LEFT JOIN clubs c ON c.id = f.club_id
                 LEFT JOIN aircraft a ON a.id = f.aircraft_id
                 JOIN pilots p ON p.id = f.pilot_id
                 JOIN LATERAL (
                     SELECT t_ms, lat, lon, alt, alt_ref, speed, course, accuracy
                     FROM fixes
                     WHERE flight_id = f.id
                     ORDER BY t_ms DESC
                     LIMIT 1
                 ) fx ON TRUE
                 WHERE f.status = 'active'
                   AND f.ended_at IS NULL
                   AND f.last_fix_at IS NOT NULL
                   AND f.last_fix_at >= $1
                   AND f.visibility = 'public'
                   -- Club-less public flights (f.club_id IS NULL) are intentionally excluded;
                   -- relax this predicate when club-less flight support is introduced.
                   AND f.club_id IS NOT NULL
                   AND f.club_id <> $2
                   AND c.status = 'live'
                   AND c.email_verified_at IS NOT NULL
                   AND c.suspended_at IS NULL
                   AND a.tail IS NOT NULL
                   AND fx.lat BETWEEN $3 AND $4
                   AND fx.lon BETWEEN $5 AND $6
             )
             SELECT flight_id,
                    tail,
                    CASE WHEN show_pilot_name_public THEN pilot_name ELSE NULL END AS pilot_name,
                    club_id,
                    trail_minutes,
                    trail_minutes_default,
                    t_ms,
                    lat,
                    lon,
                    alt,
                    alt_ref,
                    speed,
                    course,
                    accuracy
             FROM candidate_public_flights",
        )
        .bind(live_cutoff_ms)
        .bind(own_club_id)
        .bind(min_lat)
        .bind(max_lat)
        .bind(min_lon)
        .bind(max_lon)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        let dem = self.dem_index();
        let mut scored: Vec<(f64, TailLivePosition)> = rows
            .into_iter()
            .filter_map(|r| {
                let flight_id: String = r.try_get(0).ok()?;
                let tail: String = r.try_get(1).ok()?;
                let pilot_name: Option<String> = r.try_get(2).ok().flatten();
                let _club_id: Option<String> = r.try_get(3).ok().flatten();
                let flight_override: Option<i16> = r.try_get(4).ok().flatten();
                let club_default: i16 = r
                    .try_get::<Option<i16>, _>(5)
                    .ok()
                    .flatten()
                    .unwrap_or(DEFAULT_TRAIL_MINUTES);
                let t: i64 = r.try_get(6).ok()?;
                let lat: f64 = r.try_get(7).ok()?;
                let lon: f64 = r.try_get(8).ok()?;
                if !lat.is_finite() || !lon.is_finite() {
                    return None;
                }
                let dist_km = haversine_km(base_lat, base_lon, lat, lon);
                if dist_km > radius_km {
                    return None;
                }
                let fix = crate::live::LiveFix {
                    t,
                    lat,
                    lon,
                    alt: r.try_get(9).ok().flatten(),
                    alt_ref: r.try_get(10).ok().flatten(),
                    speed: r.try_get(11).ok().flatten(),
                    course: r.try_get(12).ok().flatten(),
                    accuracy: r.try_get(13).ok().flatten(),
                };
                if !matches!(
                    evaluate_public_altitude(&fix, altitude_mode, altitude_min_ft, &dem),
                    AltitudeEval::Pass | AltitudeEval::NoDemFallbackMsl
                ) {
                    return None;
                }
                let effective = TrailMinutes::effective(flight_override, club_default);
                Some((
                    dist_km,
                    TailLivePosition {
                        tail,
                        flight_id,
                        pilot_id: None,
                        pilot_name,
                        fix,
                        visibility: "public".to_string(),
                        trail_minutes: effective.as_i16(),
                        non_club: true,
                    },
                ))
            })
            .collect();

        scored.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.flight_id.cmp(&b.1.flight_id))
        });
        scored.truncate(MAX_NEARBY_PUBLIC_FLIGHTS);
        scored.into_iter().map(|(_, pos)| pos).collect()
    }

    pub async fn admin_map_scoped(&self, admin_id: &str) -> Vec<TailLivePosition> {
        let Ok(club_id) = self.pilot_club_id(admin_id).await else {
            return Vec::new();
        };
        let own_rows: Vec<DeviceFix> = self
            .latest_device_fixes()
            .await
            .into_iter()
            .filter(|r| r.club_id == club_id)
            .collect();
        let own_positions = dedupe_by_tail(own_rows);
        let prefs = self.admin_map_tails(admin_id).await;
        let mut positions = filter_admin_tails(own_positions, &prefs);
        let nearby = self
            .nearby_public_positions_for_club(&club_id, now_ms())
            .await;
        positions.extend(nearby);
        positions
    }

    pub async fn display_map_scoped(
        &self,
        display_club_id: &str,
        admin_creator_id: &str,
    ) -> Vec<TailLivePosition> {
        let own_rows: Vec<DeviceFix> = self
            .latest_device_fixes()
            .await
            .into_iter()
            .filter(|r| {
                if r.club_id != display_club_id {
                    return false;
                }
                let vis = FlightVisibility::parse(&r.visibility).unwrap_or(FlightVisibility::Club);
                matches!(vis, FlightVisibility::Club | FlightVisibility::Public)
            })
            .collect();
        let mut own_positions = dedupe_by_tail(own_rows);
        for pos in &mut own_positions {
            if let Some(f) = self.get_flight(&pos.flight_id).await {
                if show_pilot_name(&Viewer::Anonymous, &f.pilot_id, f.show_pilot_name_public) {
                    pos.pilot_name = Some(f.pilot_name);
                } else {
                    pos.pilot_name = None;
                }
            }
        }
        let prefs = self.admin_map_tails(admin_creator_id).await;
        let mut positions = filter_admin_tails(own_positions, &prefs);
        let nearby = self
            .nearby_public_positions_for_club(display_club_id, now_ms())
            .await;
        positions.extend(nearby);
        positions
    }
}
