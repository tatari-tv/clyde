use chrono::{Datelike, Local, NaiveDate, Timelike};

use crate::output::DaySummary;

/// Fraction of the current day that has elapsed (0.0 to 1.0)
pub fn day_fraction() -> f64 {
    let now = Local::now();
    let secs = now.hour() * 3600 + now.minute() * 60 + now.second();
    secs as f64 / 86400.0
}

/// Fraction of the current Sunday-based week that has elapsed (Sun-Sat)
pub fn week_fraction() -> f64 {
    let now = Local::now();
    let days_from_sunday = now.weekday().num_days_from_sunday() as f64;
    (days_from_sunday + day_fraction()) / 7.0
}

/// Fraction of the current month that has elapsed
pub fn month_fraction() -> f64 {
    let now = Local::now();
    let today = now.date_naive();
    let dim = days_in_month(today.year(), today.month());
    ((today.day() - 1) as f64 + day_fraction()) / dim as f64
}

/// Number of days in a given year/month
fn days_in_month(year: i32, month: u32) -> u32 {
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    next.expect("valid date")
        .signed_duration_since(NaiveDate::from_ymd_opt(year, month, 1).expect("valid date"))
        .num_days() as u32
}

/// Compute effective day count for averaging
pub fn effective_days(days: &[DaySummary]) -> f64 {
    let today = Local::now().date_naive();
    let mut eff = 0.0;
    for day in days {
        if day.date == today {
            eff += day_fraction();
        } else {
            eff += 1.0;
        }
    }
    eff
}

/// Compute effective week count for averaging (Sunday-based weeks)
pub fn effective_weeks(weeks: &[(String, f64, usize)]) -> f64 {
    let today = Local::now().date_naive();
    let days_since_sunday = today.weekday().num_days_from_sunday() as i64;
    let current_sunday = today - chrono::Duration::days(days_since_sunday);
    let current_key = format!("{}", current_sunday);
    let mut eff = 0.0;
    for (key, _, _) in weeks {
        if *key == current_key {
            eff += week_fraction();
        } else {
            eff += 1.0;
        }
    }
    eff
}

/// Compute effective month count for averaging
pub fn effective_months(months: &[(String, f64, usize)]) -> f64 {
    let today = Local::now().date_naive();
    let current_key = format!("{}-{:02}", today.year(), today.month());
    let mut eff = 0.0;
    for (key, _, _) in months {
        if *key == current_key {
            eff += month_fraction();
        } else {
            eff += 1.0;
        }
    }
    eff
}

/// Format the average line for text output
pub fn format_average_text(period: &str, avg: f64) -> String {
    format!("Average: ${:.2}/{}", avg, period)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_days_in_month_regular() {
        assert_eq!(days_in_month(2026, 3), 31);
        assert_eq!(days_in_month(2026, 4), 30);
        assert_eq!(days_in_month(2026, 2), 28);
    }

    #[test]
    fn test_days_in_month_leap() {
        assert_eq!(days_in_month(2024, 2), 29);
    }

    #[test]
    fn test_days_in_month_december() {
        assert_eq!(days_in_month(2026, 12), 31);
    }

    #[test]
    fn test_day_fraction_in_range() {
        let frac = day_fraction();
        assert!((0.0..=1.0).contains(&frac));
    }

    #[test]
    fn test_week_fraction_in_range() {
        let frac = week_fraction();
        assert!((0.0..=1.0).contains(&frac));
    }

    #[test]
    fn test_month_fraction_in_range() {
        let frac = month_fraction();
        assert!((0.0..=1.0).contains(&frac));
    }

    #[test]
    fn test_effective_days_no_today() {
        let days = vec![
            DaySummary {
                date: NaiveDate::from_ymd_opt(2020, 1, 1).expect("valid date"),
                cost: 10.0,
                sessions: 1,
            },
            DaySummary {
                date: NaiveDate::from_ymd_opt(2020, 1, 2).expect("valid date"),
                cost: 20.0,
                sessions: 2,
            },
        ];
        let eff = effective_days(&days);
        assert!((eff - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_effective_days_with_today() {
        let today = Local::now().date_naive();
        let days = vec![
            DaySummary {
                date: NaiveDate::from_ymd_opt(2020, 1, 1).expect("valid date"),
                cost: 10.0,
                sessions: 1,
            },
            DaySummary {
                date: today,
                cost: 5.0,
                sessions: 1,
            },
        ];
        let eff = effective_days(&days);
        // Should be 1.0 (past day) + day_fraction (today)
        assert!(eff > 1.0 && eff <= 2.0);
    }

    #[test]
    fn test_effective_weeks_no_current() {
        // Sunday-based week keys (date of the Sunday)
        let weeks = vec![
            ("2020-01-05".to_string(), 100.0, 10),
            ("2020-01-12".to_string(), 200.0, 20),
        ];
        let eff = effective_weeks(&weeks);
        assert!((eff - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_effective_months_no_current() {
        let months = vec![("2020-01".to_string(), 100.0, 10), ("2020-02".to_string(), 200.0, 20)];
        let eff = effective_months(&months);
        assert!((eff - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_format_average_text() {
        assert_eq!(format_average_text("day", 14.60), "Average: $14.60/day");
        assert_eq!(format_average_text("week", 100.0), "Average: $100.00/week");
        assert_eq!(format_average_text("month", 250.50), "Average: $250.50/month");
    }

    #[test]
    fn test_effective_days_empty() {
        let days: Vec<DaySummary> = vec![];
        let eff = effective_days(&days);
        assert!((eff - 0.0).abs() < f64::EPSILON);
    }
}
