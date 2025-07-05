// Copyright 2025 Alexandre D. Díaz
pub mod models;
pub mod utils;

pub type Pool = r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>;
