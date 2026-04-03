use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;

use crate::table;

#[derive(Debug, Clone)]
pub struct DaySummary {
    pub date: NaiveDate,
    pub cost: f64,
    pub sessions: usize,
}

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub cost: f64,
    pub entries: usize,
    pub last_active: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct TodayJson {
    pub today: f64,
    pub sessions: usize,
}

#[derive(Serialize)]
pub struct DailyJson {
    pub days: Vec<DayEntryJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_periods: Option<f64>,
}

#[derive(Serialize)]
pub struct DayEntryJson {
    pub date: String,
    pub cost: f64,
    pub sessions: usize,
}

#[derive(Serialize)]
pub struct WeeklyJson {
    pub weeks: Vec<WeekEntryJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_periods: Option<f64>,
}

#[derive(Serialize)]
pub struct WeekEntryJson {
    pub week: String,
    pub cost: f64,
    pub sessions: usize,
}

#[derive(Serialize)]
pub struct MonthlyJson {
    pub months: Vec<MonthEntryJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_periods: Option<f64>,
}

#[derive(Serialize)]
pub struct MonthEntryJson {
    pub month: String,
    pub cost: f64,
    pub sessions: usize,
}

pub fn format_today_text(summary: &DaySummary) -> String {
    format!(
        "Today: ${:.2} ({} session{})",
        summary.cost,
        summary.sessions,
        if summary.sessions == 1 { "" } else { "s" }
    )
}

pub fn format_today_json(summary: &DaySummary) -> String {
    let json = TodayJson {
        today: round_cents(summary.cost),
        sessions: summary.sessions,
    };
    serde_json::to_string(&json).unwrap_or_default()
}

#[derive(Serialize)]
pub struct YesterdayJson {
    pub yesterday: f64,
    pub sessions: usize,
}

pub fn format_yesterday_text(summary: &DaySummary) -> String {
    format!(
        "Yesterday: ${:.2} ({} session{})",
        summary.cost,
        summary.sessions,
        if summary.sessions == 1 { "" } else { "s" }
    )
}

pub fn format_yesterday_json(summary: &DaySummary) -> String {
    let json = YesterdayJson {
        yesterday: round_cents(summary.cost),
        sessions: summary.sessions,
    };
    serde_json::to_string(&json).unwrap_or_default()
}

pub fn format_daily_text(days: &[DaySummary]) -> String {
    let rows = days
        .iter()
        .map(|d| vec![d.date.to_string(), format!("${:.2}", d.cost), d.sessions.to_string()])
        .collect();
    table::build(&["Date", "Cost", "Sessions"], rows, &[1, 2])
}

pub fn format_daily_json(days: &[DaySummary], avg: Option<(f64, f64)>) -> String {
    let json = DailyJson {
        days: days
            .iter()
            .map(|d| DayEntryJson {
                date: d.date.to_string(),
                cost: round_cents(d.cost),
                sessions: d.sessions,
            })
            .collect(),
        average: avg.map(|(a, _)| round_cents(a)),
        effective_periods: avg.map(|(_, e)| round_cents(e)),
    };
    serde_json::to_string(&json).unwrap_or_default()
}

pub fn format_weekly_text(weeks: &[(String, f64, usize)]) -> String {
    let rows = weeks
        .iter()
        .map(|(w, c, s)| vec![w.clone(), format!("${:.2}", c), s.to_string()])
        .collect();
    table::build(&["Week", "Cost", "Sessions"], rows, &[1, 2])
}

pub fn format_weekly_json(weeks: &[(String, f64, usize)], avg: Option<(f64, f64)>) -> String {
    let json = WeeklyJson {
        weeks: weeks
            .iter()
            .map(|(week, cost, sessions)| WeekEntryJson {
                week: week.clone(),
                cost: round_cents(*cost),
                sessions: *sessions,
            })
            .collect(),
        average: avg.map(|(a, _)| round_cents(a)),
        effective_periods: avg.map(|(_, e)| round_cents(e)),
    };
    serde_json::to_string(&json).unwrap_or_default()
}

pub fn format_monthly_text(months: &[(String, f64, usize)]) -> String {
    let rows = months
        .iter()
        .map(|(m, c, s)| vec![m.clone(), format!("${:.2}", c), s.to_string()])
        .collect();
    table::build(&["Month", "Cost", "Sessions"], rows, &[1, 2])
}

pub fn format_monthly_json(months: &[(String, f64, usize)], avg: Option<(f64, f64)>) -> String {
    let json = MonthlyJson {
        months: months
            .iter()
            .map(|(month, cost, sessions)| MonthEntryJson {
                month: month.clone(),
                cost: round_cents(*cost),
                sessions: *sessions,
            })
            .collect(),
        average: avg.map(|(a, _)| round_cents(a)),
        effective_periods: avg.map(|(_, e)| round_cents(e)),
    };
    serde_json::to_string(&json).unwrap_or_default()
}

pub fn format_verbose_sessions(sessions: &[SessionSummary]) -> String {
    let mut out = String::new();
    for s in sessions {
        out.push_str(&format!(
            "  {}  ${:.2} ({} entries)\n",
            &s.session_id[..8.min(s.session_id.len())],
            s.cost,
            s.entries
        ));
    }
    out.trim_end().to_string()
}

fn round_cents(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_today_text() {
        let summary = DaySummary {
            date: NaiveDate::from_ymd_opt(2026, 3, 10).expect("valid date"),
            cost: 14.234,
            sessions: 3,
        };
        assert_eq!(format_today_text(&summary), "Today: $14.23 (3 sessions)");
    }

    #[test]
    fn test_format_today_text_singular() {
        let summary = DaySummary {
            date: NaiveDate::from_ymd_opt(2026, 3, 10).expect("valid date"),
            cost: 7.40,
            sessions: 1,
        };
        assert_eq!(format_today_text(&summary), "Today: $7.40 (1 session)");
    }

    #[test]
    fn test_format_today_json() {
        let summary = DaySummary {
            date: NaiveDate::from_ymd_opt(2026, 3, 10).expect("valid date"),
            cost: 14.236,
            sessions: 3,
        };
        let json = format_today_json(&summary);
        assert!(json.contains("\"today\":14.24"));
        assert!(json.contains("\"sessions\":3"));
    }

    #[test]
    fn test_format_yesterday_text() {
        let summary = DaySummary {
            date: NaiveDate::from_ymd_opt(2026, 3, 10).expect("valid date"),
            cost: 22.175,
            sessions: 5,
        };
        assert_eq!(format_yesterday_text(&summary), "Yesterday: $22.18 (5 sessions)");
    }

    #[test]
    fn test_format_yesterday_text_singular() {
        let summary = DaySummary {
            date: NaiveDate::from_ymd_opt(2026, 3, 10).expect("valid date"),
            cost: 3.00,
            sessions: 1,
        };
        assert_eq!(format_yesterday_text(&summary), "Yesterday: $3.00 (1 session)");
    }

    #[test]
    fn test_format_yesterday_json() {
        let summary = DaySummary {
            date: NaiveDate::from_ymd_opt(2026, 3, 10).expect("valid date"),
            cost: 22.176,
            sessions: 5,
        };
        let json = format_yesterday_json(&summary);
        assert!(json.contains("\"yesterday\":22.18"));
        assert!(json.contains("\"sessions\":5"));
    }

    #[test]
    fn test_format_daily_text() {
        let days = vec![
            DaySummary {
                date: NaiveDate::from_ymd_opt(2026, 3, 10).expect("valid date"),
                cost: 14.23,
                sessions: 3,
            },
            DaySummary {
                date: NaiveDate::from_ymd_opt(2026, 3, 9).expect("valid date"),
                cost: 22.17,
                sessions: 5,
            },
        ];
        let text = format_daily_text(&days);
        assert!(text.contains("Date"));
        assert!(text.contains("Cost"));
        assert!(text.contains("Sessions"));
        assert!(text.contains("2026-03-10"));
        assert!(text.contains("14.23"));
        assert!(text.contains("2026-03-09"));
        // No parentheses or pluralization
        assert!(!text.contains("session)"));
        assert!(!text.contains("sessions)"));
    }

    #[test]
    fn test_format_weekly_text() {
        let weeks = vec![
            ("2026-03-08".to_string(), 47.82, 12),
            ("2026-03-01".to_string(), 123.45, 28),
        ];
        let text = format_weekly_text(&weeks);
        assert!(text.contains("Week"));
        assert!(text.contains("Cost"));
        assert!(text.contains("Sessions"));
        assert!(text.contains("2026-03-08"));
        assert!(text.contains("47.82"));
        assert!(text.contains("2026-03-01"));
        assert!(text.contains("123.45"));
        // No parentheses or pluralization
        assert!(!text.contains("session)"));
        assert!(!text.contains("sessions)"));
    }

    #[test]
    fn test_format_weekly_json() {
        let weeks = vec![
            ("2026-03-08".to_string(), 47.826, 12),
            ("2026-03-01".to_string(), 123.454, 28),
        ];
        let json = format_weekly_json(&weeks, None);
        assert!(json.contains("\"week\":\"2026-03-08\""));
        assert!(json.contains("\"cost\":47.83"));
        assert!(json.contains("\"sessions\":12"));
        assert!(json.contains("\"week\":\"2026-03-01\""));
        assert!(json.contains("\"cost\":123.45"));
    }

    #[test]
    fn test_format_weekly_json_empty() {
        let weeks: Vec<(String, f64, usize)> = vec![];
        let json = format_weekly_json(&weeks, None);
        assert_eq!(json, "{\"weeks\":[]}");
    }

    #[test]
    fn test_round_cents() {
        assert!((round_cents(14.236) - 14.24).abs() < f64::EPSILON);
        assert!((round_cents(14.234) - 14.23).abs() < f64::EPSILON);
    }
}
