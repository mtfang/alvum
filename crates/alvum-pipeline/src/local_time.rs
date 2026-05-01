#[cfg(test)]
use chrono::FixedOffset;
use chrono::{DateTime, Local, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalTimeContext {
    pub timezone_label: String,
    pub utc_offset: String,
    pub generated_at_utc: String,
    pub generated_at_local: String,
}

impl LocalTimeContext {
    pub fn now() -> Self {
        let generated_at_utc = Utc::now();
        let generated_at_local = generated_at_utc.with_timezone(&Local);
        Self {
            timezone_label: generated_at_local.format("%Z").to_string(),
            utc_offset: generated_at_local.format("%:z").to_string(),
            generated_at_utc: generated_at_utc.to_rfc3339(),
            generated_at_local: generated_at_local.to_rfc3339(),
        }
    }
}

pub fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

pub fn format_date(ts: DateTime<Utc>) -> String {
    ts.with_timezone(&Local).format("%Y-%m-%d").to_string()
}

pub fn format_hm(ts: DateTime<Utc>) -> String {
    ts.with_timezone(&Local).format("%H:%M").to_string()
}

pub fn format_hms(ts: DateTime<Utc>) -> String {
    ts.with_timezone(&Local).format("%H:%M:%S").to_string()
}

pub fn format_rfc3339(ts: DateTime<Utc>) -> String {
    ts.with_timezone(&Local).to_rfc3339()
}

pub fn parse_decision_datetime(date: &str, time: &str) -> Option<DateTime<Utc>> {
    let naive = parse_naive_decision_datetime(date, time)?;
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Some(dt.with_timezone(&Utc)),
        LocalResult::Ambiguous(earlier, _) => Some(earlier.with_timezone(&Utc)),
        LocalResult::None => None,
    }
}

#[cfg(test)]
pub(crate) fn format_date_with_offset(ts: DateTime<Utc>, offset: FixedOffset) -> String {
    ts.with_timezone(&offset).format("%Y-%m-%d").to_string()
}

#[cfg(test)]
pub(crate) fn format_hm_with_offset(ts: DateTime<Utc>, offset: FixedOffset) -> String {
    ts.with_timezone(&offset).format("%H:%M").to_string()
}

#[cfg(test)]
pub(crate) fn format_hms_with_offset(ts: DateTime<Utc>, offset: FixedOffset) -> String {
    ts.with_timezone(&offset).format("%H:%M:%S").to_string()
}

#[cfg(test)]
pub(crate) fn parse_decision_datetime_with_offset(
    date: &str,
    time: &str,
    offset: FixedOffset,
) -> Option<DateTime<Utc>> {
    let naive = parse_naive_decision_datetime(date, time)?;
    match offset.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Some(dt.with_timezone(&Utc)),
        LocalResult::Ambiguous(earlier, _) => Some(earlier.with_timezone(&Utc)),
        LocalResult::None => None,
    }
}

fn parse_naive_decision_datetime(date: &str, time: &str) -> Option<NaiveDateTime> {
    let date = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").ok()?;
    let time = NaiveTime::parse_from_str(time.trim(), "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(time.trim(), "%H:%M:%S"))
        .ok()?;
    Some(date.and_time(time))
}
