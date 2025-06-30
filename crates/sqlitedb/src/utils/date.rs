// Copyright 2025 Alexandre D. Díaz
use chrono::{DateTime, Utc};

pub fn to_sqlite_datetime(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn get_sqlite_utc_now() -> String {
    to_sqlite_datetime(Utc::now())
}