use chrono::{DateTime, NaiveDate, TimeZone, Utc};

/// Get the local date for a UTC timestamp.
///
/// Lives in ccu (not the library) because day-bucketing by user-local date is a
/// ccu reporting concern; the library's parse layer surfaces UTC timestamps
/// without imposing a calendar.
pub fn local_date(ts: &DateTime<Utc>) -> NaiveDate {
    let local = chrono::Local.from_utc_datetime(&ts.naive_utc());
    local.date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    #[test]
    fn test_local_date() {
        let ts: DateTime<Utc> = "2026-03-10T14:23:01.025Z".parse().expect("parse");
        let date = local_date(&ts);
        assert!(date.month() == 3);
    }
}
