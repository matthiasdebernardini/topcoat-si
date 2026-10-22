//! Conversions between this crate's `time::OffsetDateTime` and SurrealDB's `Datetime`.

use surrealdb::types::Datetime;
use time::OffsetDateTime;

/// A SurrealDB datetime for `at`, to the nanosecond.
pub(crate) fn to_datetime(at: OffsetDateTime) -> Datetime {
    // `from_timestamp` fails only outside chrono's range of about plus or minus 262,000 years,
    // which `OffsetDateTime` cannot reach either.
    Datetime::from_timestamp(at.unix_timestamp(), at.nanosecond()).unwrap_or_default()
}

/// The `OffsetDateTime` for a SurrealDB datetime, in UTC.
pub(crate) fn from_datetime(at: &Datetime) -> OffsetDateTime {
    let seconds = at.timestamp();
    let nanos = at.timestamp_subsec_nanos();
    OffsetDateTime::from_unix_timestamp(seconds)
        .map(|t| t + time::Duration::nanoseconds(i64::from(nanos)))
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_to_the_nanosecond() {
        let at = OffsetDateTime::from_unix_timestamp_nanos(1_761_000_000_123_456_789).unwrap();
        assert_eq!(from_datetime(&to_datetime(at)), at);
    }

    #[test]
    fn matches_the_string_form() {
        let at = OffsetDateTime::from_unix_timestamp(1_761_000_000).unwrap();
        assert_eq!(to_datetime(at).to_string(), "2025-10-20T22:40:00Z");
    }
}
