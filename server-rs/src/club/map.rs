use crate::live::LiveFix;
use serde::Serialize;
use std::collections::HashMap;

const ACC_GOOD_M: f64 = 50.0;
const ACC_WINDOW_MS: i64 = 5_000;

pub const TAIL_SOURCE_CLUB: &str = "club";
pub const TAIL_SOURCE_PILOT: &str = "pilot";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TailLivePosition {
    pub tail: String,
    pub tail_source: &'static str,
    pub flight_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pilot_id: Option<String>,
    pub pilot_name: Option<String>,
    pub fix: LiveFix,
    pub visibility: String,
    pub trail_minutes: i16,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub non_club: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub club_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub club_slug: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DeviceFix {
    pub tail: String,
    pub flight_id: String,
    pub pilot_id: String,
    pub pilot_name: String,
    pub visibility: String,
    pub club_id: Option<String>,
    pub is_club_aircraft: bool,
    pub clubless_public_eligible: bool,
    pub last_fix_at: Option<i64>,
    pub trail_minutes: i16,
    pub fix: LiveFix,
}

/// One aircraft per tail: best accuracy within 5s among device fixes.
pub fn dedupe_by_tail(inputs: Vec<DeviceFix>) -> Vec<TailLivePosition> {
    let mut by_tail: HashMap<String, Vec<DeviceFix>> = HashMap::new();
    for row in inputs {
        by_tail.entry(row.tail.clone()).or_default().push(row);
    }
    let mut out = Vec::new();
    for (_tail, mut rows) in by_tail {
        rows.sort_by_key(|r| std::cmp::Reverse(r.fix.t));
        let latest_t = rows.first().map(|r| r.fix.t).unwrap_or(0);
        let window_start = latest_t - ACC_WINDOW_MS;
        let mut best: Option<&DeviceFix> = None;
        let mut best_acc = f64::MAX;
        for row in rows.iter().filter(|r| r.fix.t >= window_start) {
            let acc = row.fix.accuracy.unwrap_or(9999.0);
            if acc <= ACC_GOOD_M && acc < best_acc {
                best_acc = acc;
                best = Some(row);
            }
        }
        let pick = best.or_else(|| rows.first());
        if let Some(row) = pick {
            out.push(TailLivePosition {
                tail: row.tail.clone(),
                tail_source: if row.is_club_aircraft {
                    TAIL_SOURCE_CLUB
                } else {
                    TAIL_SOURCE_PILOT
                },
                flight_id: row.flight_id.clone(),
                pilot_id: Some(row.pilot_id.clone()),
                pilot_name: Some(row.pilot_name.clone()),
                fix: row.fix.clone(),
                visibility: row.visibility.clone(),
                trail_minutes: row.trail_minutes,
                non_club: false,
                club_name: None,
                club_slug: None,
            });
        }
    }
    out.sort_by(|a, b| a.tail.cmp(&b.tail));
    out
}

/// Admin display map only: filter by selected tail numbers (empty = show all).
pub fn filter_admin_tails(
    positions: Vec<TailLivePosition>,
    tails: &[String],
) -> Vec<TailLivePosition> {
    if tails.is_empty() {
        return positions;
    }
    let set: std::collections::HashSet<_> = tails.iter().map(String::as_str).collect();
    positions
        .into_iter()
        .filter(|p| set.contains(p.tail.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::LiveFix;
    use super::*;

    fn fix(t: i64, acc: Option<f64>) -> LiveFix {
        LiveFix {
            t,
            lat: -33.4,
            lon: -70.6,
            alt: Some(800.0),
            alt_ref: Some("wgs84_ellipsoid".to_string()),
            speed: None,
            course: None,
            accuracy: acc,
        }
    }

    fn row(tail: &str, t: i64, acc: Option<f64>) -> DeviceFix {
        DeviceFix {
            tail: tail.to_string(),
            flight_id: "f1".to_string(),
            pilot_id: "p1".to_string(),
            pilot_name: "Pat".to_string(),
            visibility: "club".to_string(),
            club_id: Some("c1".to_string()),
            is_club_aircraft: true,
            clubless_public_eligible: false,
            last_fix_at: Some(t),
            trail_minutes: 5,
            fix: fix(t, acc),
        }
    }

    #[test]
    fn dedupe_prefers_better_accuracy_in_window() {
        let out = dedupe_by_tail(vec![
            row("N123", 1000, Some(80.0)),
            row("N123", 1000, Some(10.0)),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].fix.accuracy, Some(10.0));
        assert_eq!(out[0].tail_source, TAIL_SOURCE_CLUB);
    }

    #[test]
    fn admin_filter_does_not_drop_unselected_from_source() {
        let all = vec![
            TailLivePosition {
                tail: "A".to_string(),
                tail_source: TAIL_SOURCE_CLUB,
                flight_id: "1".into(),
                pilot_id: Some("p".into()),
                pilot_name: None,
                fix: fix(1, Some(5.0)),
                visibility: "club".into(),
                trail_minutes: 5,
                non_club: false,
                club_name: None,
                club_slug: None,
            },
            TailLivePosition {
                tail: "B".to_string(),
                tail_source: TAIL_SOURCE_CLUB,
                flight_id: "2".into(),
                pilot_id: Some("p".into()),
                pilot_name: None,
                fix: fix(1, Some(5.0)),
                visibility: "club".into(),
                trail_minutes: 5,
                non_club: false,
                club_name: None,
                club_slug: None,
            },
        ];
        let filtered = filter_admin_tails(all.clone(), &["A".to_string()]);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].tail, "A");
        assert_eq!(all.len(), 2);
    }
}
