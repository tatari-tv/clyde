use crate::output::DaySummary;
use crate::table;

// Left-fractional blocks: index 0 = empty, 1 = 1/8th, ..., 8 = full block
const BLOCKS: [char; 9] = [
    ' ',        // 0/8
    '\u{258F}', // 1/8  Left One Eighth Block
    '\u{258E}', // 2/8  Left One Quarter Block
    '\u{258D}', // 3/8  Left Three Eighths Block
    '\u{258C}', // 4/8  Left Half Block
    '\u{258B}', // 5/8  Left Five Eighths Block
    '\u{258A}', // 6/8  Left Three Quarters Block
    '\u{2589}', // 7/8  Left Seven Eighths Block
    '\u{2588}', // 8/8  Full Block
];

/// Render an inline Unicode horizontal bar
pub fn bar(value: f64, max_value: f64, max_width: usize) -> String {
    if max_value <= 0.0 || value <= 0.0 {
        return String::new();
    }
    let ratio = (value / max_value).min(1.0);
    let total_eighths = (ratio * max_width as f64 * 8.0) as usize;
    let full_blocks = total_eighths / 8;
    let remainder = total_eighths % 8;
    let mut out = String::new();
    for _ in 0..full_blocks {
        out.push(BLOCKS[8]);
    }
    if remainder > 0 {
        out.push(BLOCKS[remainder]);
    }
    out
}

/// Format daily text output with inline bars
pub fn format_daily_text_with_bars(days: &[DaySummary]) -> String {
    let max_cost = days.iter().map(|d| d.cost).fold(0.0_f64, f64::max);
    let rows = days
        .iter()
        .map(|d| {
            vec![
                d.date.to_string(),
                format!("${:.2}", d.cost),
                d.sessions.to_string(),
                bar(d.cost, max_cost, 20),
            ]
        })
        .collect();
    table::build(&["Date", "Cost", "Sessions", "Graph"], rows, &[1, 2])
}

/// Format weekly text output with inline bars
pub fn format_weekly_text_with_bars(weeks: &[(String, f64, usize)]) -> String {
    let max_cost = weeks.iter().map(|(_, c, _)| *c).fold(0.0_f64, f64::max);
    let rows = weeks
        .iter()
        .map(|(w, c, s)| vec![w.clone(), format!("${:.2}", c), s.to_string(), bar(*c, max_cost, 20)])
        .collect();
    table::build(&["Week", "Cost", "Sessions", "Graph"], rows, &[1, 2])
}

/// Format monthly text output with inline bars
pub fn format_monthly_text_with_bars(months: &[(String, f64, usize)]) -> String {
    let max_cost = months.iter().map(|(_, c, _)| *c).fold(0.0_f64, f64::max);
    let rows = months
        .iter()
        .map(|(m, c, s)| vec![m.clone(), format!("${:.2}", c), s.to_string(), bar(*c, max_cost, 20)])
        .collect();
    table::build(&["Month", "Cost", "Sessions", "Graph"], rows, &[1, 2])
}

/// Render a sparkline trend indicator from cost data.
/// Input is newest-first; sparkline is displayed in chronological order.
pub fn render_sparkline(costs: &[f64]) -> String {
    if costs.is_empty() {
        return String::new();
    }
    let chronological: Vec<f64> = costs.iter().rev().copied().collect();
    format!("Trend: {}", sparklines::spark(&chronological))
}

/// Render a Unicode box-drawing line chart from cost data.
/// Returns None when < 3 data points or on error.
/// Input is newest-first; chart is displayed in chronological order.
pub fn render_chart(costs: &[f64]) -> Option<String> {
    if costs.len() < 3 {
        return None;
    }
    let chronological: Vec<f64> = costs.iter().rev().copied().collect();
    let config = rasciichart::Config {
        height: 7,
        label_format: "${:.2}".to_string(),
        ..rasciichart::Config::default()
    };
    rasciichart::plot_with_config(&chronological, config).ok()
}

/// Render a line chart for daily data
pub fn daily_chart(days: &[DaySummary]) -> Option<String> {
    let costs: Vec<f64> = days.iter().map(|d| d.cost).collect();
    render_chart(&costs)
}

/// Render a sparkline for daily data
pub fn daily_sparkline(days: &[DaySummary]) -> String {
    let costs: Vec<f64> = days.iter().map(|d| d.cost).collect();
    render_sparkline(&costs)
}

/// Render a line chart for weekly data
pub fn weekly_chart(weeks: &[(String, f64, usize)]) -> Option<String> {
    let costs: Vec<f64> = weeks.iter().map(|(_, c, _)| *c).collect();
    render_chart(&costs)
}

/// Render a sparkline for weekly data
pub fn weekly_sparkline(weeks: &[(String, f64, usize)]) -> String {
    let costs: Vec<f64> = weeks.iter().map(|(_, c, _)| *c).collect();
    render_sparkline(&costs)
}

/// Render a line chart for monthly data
pub fn monthly_chart(months: &[(String, f64, usize)]) -> Option<String> {
    let costs: Vec<f64> = months.iter().map(|(_, c, _)| *c).collect();
    render_chart(&costs)
}

/// Render a sparkline for monthly data
pub fn monthly_sparkline(months: &[(String, f64, usize)]) -> String {
    let costs: Vec<f64> = months.iter().map(|(_, c, _)| *c).collect();
    render_sparkline(&costs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn test_bar_zero_value() {
        assert_eq!(bar(0.0, 100.0, 20), "");
    }

    #[test]
    fn test_bar_zero_max() {
        assert_eq!(bar(50.0, 0.0, 20), "");
    }

    #[test]
    fn test_bar_max_value() {
        let result = bar(100.0, 100.0, 20);
        assert_eq!(result.chars().count(), 20);
        assert!(result.chars().all(|c| c == '\u{2588}'));
    }

    #[test]
    fn test_bar_half_value() {
        let result = bar(50.0, 100.0, 20);
        assert_eq!(result.chars().count(), 10);
    }

    #[test]
    fn test_bar_fractional() {
        let result = bar(25.0, 100.0, 20);
        assert_eq!(result.chars().count(), 5);
    }

    #[test]
    fn test_format_daily_text_with_bars() {
        let days = vec![
            DaySummary {
                date: NaiveDate::from_ymd_opt(2026, 3, 10).expect("valid date"),
                cost: 20.0,
                sessions: 3,
            },
            DaySummary {
                date: NaiveDate::from_ymd_opt(2026, 3, 9).expect("valid date"),
                cost: 10.0,
                sessions: 1,
            },
        ];
        let text = format_daily_text_with_bars(&days);
        assert!(text.contains("Date"));
        assert!(text.contains("Graph"));
        assert!(text.contains("2026-03-10"));
        assert!(text.contains("2026-03-09"));
        assert!(text.contains('\u{2588}'));
        // No parentheses
        assert!(!text.contains("session)"));
    }

    #[test]
    fn test_render_chart_returns_none_for_few_points() {
        assert!(render_chart(&[]).is_none());
        assert!(render_chart(&[10.0]).is_none());
        assert!(render_chart(&[10.0, 20.0]).is_none());
    }

    #[test]
    fn test_render_chart_returns_some_for_enough_points() {
        let result = render_chart(&[30.0, 20.0, 10.0]);
        assert!(result.is_some());
        let chart = result.expect("chart should be Some");
        assert!(!chart.is_empty());
    }

    #[test]
    fn test_render_sparkline() {
        let sparkline = render_sparkline(&[30.0, 20.0, 10.0]);
        assert!(sparkline.starts_with("Trend: "));
        assert!(sparkline.len() > 7);
    }

    #[test]
    fn test_render_sparkline_empty() {
        assert_eq!(render_sparkline(&[]), "");
    }

    #[test]
    fn test_daily_chart_few_points() {
        let days = vec![
            DaySummary {
                date: NaiveDate::from_ymd_opt(2026, 3, 10).expect("valid date"),
                cost: 20.0,
                sessions: 3,
            },
            DaySummary {
                date: NaiveDate::from_ymd_opt(2026, 3, 9).expect("valid date"),
                cost: 10.0,
                sessions: 1,
            },
        ];
        assert!(daily_chart(&days).is_none());
    }

    #[test]
    fn test_weekly_chart_few_points() {
        let weeks = vec![
            ("2026-03-08".to_string(), 47.82, 12),
            ("2026-03-01".to_string(), 123.45, 28),
        ];
        assert!(weekly_chart(&weeks).is_none());
    }

    #[test]
    fn test_monthly_chart_few_points() {
        let months = vec![("2026-03".to_string(), 200.0, 30), ("2026-02".to_string(), 150.0, 25)];
        assert!(monthly_chart(&months).is_none());
    }
}
