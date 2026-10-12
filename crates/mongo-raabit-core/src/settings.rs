//! The user's settings of Mongo Raabit, as the host stores them (key → JSON value).
//!
//! The host renders and persists them; the views read them every frame with
//! [`Settings::from_values`]. Unset or invalid values fall back to the defaults.

use serde_json::Value;

/// Ask before every change to the database, showing the query and what it affects.
pub const CONFIRM_WRITES: &str = "confirm_writes";
/// Open every new connection read-only, whatever its profile says.
pub const NEW_CONNECTIONS_READ_ONLY: &str = "new_connections_read_only";
/// Documents per page of a new Query tab.
pub const PAGE_SIZE: &str = "page_size";
/// Documents the Schema Explorer samples by default.
pub const SCHEMA_SAMPLE_SIZE: &str = "schema_sample_size";
/// Seconds a query, an aggregation or a shell script may run.
pub const QUERY_TIMEOUT_SECS: &str = "query_timeout_secs";

/// Smallest and largest page size.
pub const PAGE_SIZE_RANGE: (u32, u32) = (1, 1000);
/// Smallest and largest schema sample.
pub const SCHEMA_SAMPLE_RANGE: (u32, u32) = (1, 100_000);
/// Shortest and longest query timeout, in seconds.
pub const QUERY_TIMEOUT_RANGE: (u32, u32) = (1, 3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub confirm_writes: bool,
    pub new_connections_read_only: bool,
    pub page_size: u32,
    pub schema_sample_size: u32,
    pub query_timeout_secs: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            confirm_writes: true,
            new_connections_read_only: false,
            page_size: 50,
            schema_sample_size: 1000,
            query_timeout_secs: 60,
        }
    }
}

fn number(value: Option<Value>, (min, max): (u32, u32), default: u32) -> u32 {
    value
        .and_then(|v| v.as_f64())
        .filter(|n| n.is_finite())
        .map_or(default, |n| {
            n.round().clamp(f64::from(min), f64::from(max)) as u32
        })
}

impl Settings {
    /// Settings from the host's values (`get` returns the stored value of a key, or the
    /// default its settings page declares). Missing or mistyped values use the defaults;
    /// numbers are clamped to their ranges.
    pub fn from_values(get: impl Fn(&str) -> Option<Value>) -> Self {
        let defaults = Self::default();
        let flag = |key: &str, default: bool| get(key).and_then(|v| v.as_bool()).unwrap_or(default);
        Self {
            confirm_writes: flag(CONFIRM_WRITES, defaults.confirm_writes),
            new_connections_read_only: flag(
                NEW_CONNECTIONS_READ_ONLY,
                defaults.new_connections_read_only,
            ),
            page_size: number(get(PAGE_SIZE), PAGE_SIZE_RANGE, defaults.page_size),
            schema_sample_size: number(
                get(SCHEMA_SAMPLE_SIZE),
                SCHEMA_SAMPLE_RANGE,
                defaults.schema_sample_size,
            ),
            query_timeout_secs: number(
                get(QUERY_TIMEOUT_SECS),
                QUERY_TIMEOUT_RANGE,
                defaults.query_timeout_secs,
            ),
        }
    }

    pub fn query_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.query_timeout_secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn defaults_ask_before_writes() {
        let settings = Settings::default();
        assert!(settings.confirm_writes);
        assert!(!settings.new_connections_read_only);
        assert_eq!(settings.page_size, 50);
        assert_eq!(settings.schema_sample_size, 1000);
        assert_eq!(settings.query_timeout_secs, 60);
        assert_eq!(Settings::from_values(|_| None), settings);
    }

    #[test]
    fn reads_host_values_and_clamps_numbers() {
        let values: HashMap<&str, Value> = HashMap::from([
            (CONFIRM_WRITES, json!(false)),
            (NEW_CONNECTIONS_READ_ONLY, json!(true)),
            // The host stores numbers as floats.
            (PAGE_SIZE, json!(100.0)),
            (SCHEMA_SAMPLE_SIZE, json!(5_000_000)),
            (QUERY_TIMEOUT_SECS, json!("soon")),
        ]);
        let settings = Settings::from_values(|key| values.get(key).cloned());
        assert_eq!(
            settings,
            Settings {
                confirm_writes: false,
                new_connections_read_only: true,
                page_size: 100,
                schema_sample_size: 100_000,
                query_timeout_secs: 60,
            }
        );
        assert_eq!(
            Settings::from_values(|key| (key == PAGE_SIZE).then(|| json!(0))).page_size,
            1
        );
        assert_eq!(settings.query_timeout().as_secs(), 60);
    }
}
