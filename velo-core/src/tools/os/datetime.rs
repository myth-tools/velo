use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeDelta, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{exec_err, ToolOutput};

#[derive(Serialize, Deserialize, Debug)]
pub struct DateTimeArgs {
    pub action: Option<String>,
    pub format: Option<String>,
    pub timestamp_s: Option<i64>,
    pub timestamp_ms: Option<i64>,
    pub datetime: Option<String>,
    pub ts1: Option<i64>,
    pub ts2: Option<i64>,
    pub unit: Option<String>,
}

impl ToolInputT for DateTimeArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"action":{"type":"string","description":"Action: 'now' (default) — current time; 'format' — format a timestamp; 'parse' — parse a datetime string; 'timestamp' — get current Unix timestamp; 'utc_to_local' — convert UTC timestamp to local; 'local_to_utc' — convert local datetime string to UTC; 'diff' — difference between two timestamps."},"format":{"type":"string","description":"Output format using strftime syntax, e.g. '%%Y-%%m-%%d %%H:%%M:%%S %%z'. Default: '%%Y-%%m-%%d %%H:%%M:%%S'."},"timestamp_s":{"type":"integer","description":"Unix timestamp in seconds (UTC). Used by: format, utc_to_local."},"timestamp_ms":{"type":"integer","description":"Unix timestamp in milliseconds (alternative to timestamp_s). Takes priority if both are set."},"datetime":{"type":"string","description":"Datetime string to parse. Accepted formats: 'YYYY-MM-DD HH:MM:SS', 'YYYY-MM-DDTHH:MM:SS', 'YYYY-MM-DD', ISO 8601/RFC 3339 with offset."},"ts1":{"type":"integer","description":"First Unix timestamp (seconds) for diff action."},"ts2":{"type":"integer","description":"Second Unix timestamp (seconds) for diff action. Result = ts2 - ts1."},"unit":{"type":"string","description":"Output unit for diff: 'auto' (default, picks largest unit), 'seconds', 'minutes', 'hours', 'days'."}}}"#
    }
}

#[tool(name = "date_time", description = "Get current date/time, format/parse Unix timestamps, convert between timezones, compute durations between timestamps. Supports ISO 8601 / RFC 3339. Actions: now, format, parse, timestamp, utc_to_local, local_to_utc, diff. BEST FOR: timestamp conversion, log file analysis, date arithmetic, displaying time in user's local timezone.", input = DateTimeArgs)]
#[derive(Default, Clone)]
pub struct DateTimeTool;

fn get_ts(a: &DateTimeArgs) -> i64 {
    a.timestamp_ms
        .map(|ms| ms / 1000)
        .or(a.timestamp_s)
        .unwrap_or_else(|| Utc::now().timestamp())
}

fn parse_naive(s: &str) -> Result<NaiveDateTime, String> {
    let trimmed = s.trim();
    // Try ISO 8601 with timezone offset (e.g. 2024-01-15T10:30:00+05:00)
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Ok(dt.naive_utc());
    }
    if let Ok(dt) = DateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S %z") {
        return Ok(dt.naive_utc());
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S") {
        return Ok(dt);
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S") {
        return Ok(dt);
    }
    if let Ok(d) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        return Ok(d.and_hms_opt(0, 0, 0).unwrap());
    }
    // ISO 8601 with Z suffix
    if let Ok(dt) = DateTime::parse_from_rfc3339(&format!("{trimmed}Z")) {
        return Ok(dt.naive_utc());
    }
    Err(format!(
        "Cannot parse '{s}'. Expected formats: YYYY-MM-DD HH:MM:SS, ISO 8601, RFC 3339"
    ))
}

#[async_trait]
impl ToolRuntime for DateTimeTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: DateTimeArgs = serde_json::from_value(args)?;
        let action = a.action.as_deref().unwrap_or("now");
        let fmt = a.format.as_deref().unwrap_or("%Y-%m-%d %H:%M:%S");

        match action {
            "now" => {
                let local = Local::now();
                let utc = Utc::now();
                let local_fmt = local.format(fmt).to_string();
                let utc_fmt = utc.format(fmt).to_string();
                let tz = local.offset();
                let ts = utc.timestamp();
                let ts_ms = utc.timestamp_millis();
                Ok(ToolOutput::ok(format!(
                    "Local: {local_fmt}\nUTC:   {utc_fmt}\nOffset: {tz}\nUnix:  {ts}s ({ts_ms}ms)"
                ))
                .into())
            }
            "timestamp" => {
                let ts = Utc::now().timestamp();
                let ts_ms = Utc::now().timestamp_millis();
                let local = Local::now().format(fmt).to_string();
                Ok(ToolOutput::ok(format!("Unix: {ts}s ({ts_ms}ms)\nLocal time: {local}")).into())
            }
            "format" => {
                let ts = get_ts(&a);
                let dt =
                    DateTime::from_timestamp(ts, 0).ok_or_else(|| exec_err("Invalid timestamp"))?;
                let local: DateTime<Local> = dt.into();
                let formatted = local.format(fmt).to_string();
                let utc_formatted = dt.format(fmt).to_string();
                Ok(ToolOutput::ok(format!("Local: {formatted}\nUTC:   {utc_formatted}")).into())
            }
            "parse" => {
                let dt_str = a
                    .datetime
                    .as_deref()
                    .ok_or_else(|| exec_err("datetime required"))?;
                let parsed = parse_naive(dt_str).map_err(exec_err)?;
                let formatted = parsed.format(fmt).to_string();
                let ts = parsed.and_utc().timestamp();
                Ok(ToolOutput::ok(format!("Parsed: {formatted}\nUnix: {ts}")).into())
            }
            "utc_to_local" => {
                let ts = a
                    .timestamp_s
                    .ok_or_else(|| exec_err("timestamp_s required"))?;
                let dt =
                    DateTime::from_timestamp(ts, 0).ok_or_else(|| exec_err("Invalid timestamp"))?;
                let local: DateTime<Local> = dt.into();
                let formatted = local.format(fmt).to_string();
                Ok(ToolOutput::ok(format!("UTC {ts} → Local: {formatted}")).into())
            }
            "local_to_utc" => {
                let dt_str = a
                    .datetime
                    .as_deref()
                    .ok_or_else(|| exec_err("datetime string (YYYY-MM-DD HH:MM:SS) required"))?;
                let naive = NaiveDateTime::parse_from_str(dt_str, "%Y-%m-%d %H:%M:%S")
                    .or_else(|_| NaiveDateTime::parse_from_str(dt_str, "%Y-%m-%dT%H:%M:%S"))
                    .map_err(|e| {
                        exec_err(format!("Cannot parse local datetime '{dt_str}': {e}"))
                    })?;
                let local_dt: DateTime<Local> = Local
                    .from_local_datetime(&naive)
                    .single()
                    .ok_or_else(|| exec_err("Ambiguous or invalid local time"))?;
                let utc_dt = local_dt.to_utc();
                let ts = utc_dt.timestamp();
                let formatted = utc_dt.format(fmt).to_string();
                Ok(
                    ToolOutput::ok(format!("Local '{dt_str}' → UTC: {formatted}\nUnix: {ts}"))
                        .into(),
                )
            }
            "diff" => {
                let ts1 = a.ts1.ok_or_else(|| exec_err("ts1 required for diff"))?;
                let ts2 = a.ts2.ok_or_else(|| exec_err("ts2 required for diff"))?;
                let diff = TimeDelta::seconds(ts2 - ts1);
                let abs_diff = diff.num_seconds().unsigned_abs();
                let unit = a.unit.as_deref().unwrap_or("auto");

                let result = match unit {
                    "seconds" => format!("{} seconds", diff.num_seconds()),
                    "minutes" => format!("{} minutes", diff.num_minutes()),
                    "hours" => format!("{} hours", diff.num_hours()),
                    "days" => format!("{} days", diff.num_days()),
                    _ => {
                        // auto: pick largest unit
                        let days = diff.num_days();
                        if days.abs() > 0 {
                            let hours = diff.num_hours() - days * 24;
                            format!("{days}d {hours}h ({abs_diff}s)")
                        } else {
                            let hours = diff.num_hours();
                            if hours.abs() > 0 {
                                let mins = diff.num_minutes() - hours * 60;
                                format!("{hours}h {mins}m ({abs_diff}s)")
                            } else {
                                let mins = diff.num_minutes();
                                if mins.abs() > 0 {
                                    let secs = diff.num_seconds() - mins * 60;
                                    format!("{mins}m {secs}s")
                                } else {
                                    format!("{abs_diff}s")
                                }
                            }
                        }
                    }
                };
                Ok(ToolOutput::ok(format!("Difference (ts2 - ts1): {result}")).into())
            }
            other => Err(exec_err(format!("Unknown action '{other}'"))),
        }
    }
}
