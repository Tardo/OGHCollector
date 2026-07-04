// Copyright Alexandre D. Díaz
pub mod models;
pub mod schema;
pub mod utils;

use diesel::r2d2::{ConnectionManager, CustomizeConnection, Error as R2d2Error, Pool as R2d2Pool};
use diesel::sqlite::SqliteConnection;
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};

pub use diesel::r2d2::PooledConnection;
pub use diesel::sqlite::SqliteConnection as DbSqliteConnection;

pub type Pool = R2d2Pool<ConnectionManager<SqliteConnection>>;
pub type Connection = PooledConnection<ConnectionManager<SqliteConnection>>;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("../../migrations");

pub fn run_migrations(
    conn: &mut SqliteConnection,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    conn.run_pending_migrations(MIGRATIONS)?;
    Ok(())
}

// Customizes connections for write access (WAL mode, timeouts).
#[derive(Debug)]
struct WriteCustomizer;

impl CustomizeConnection<SqliteConnection, R2d2Error> for WriteCustomizer {
    fn on_acquire(&self, conn: &mut SqliteConnection) -> Result<(), R2d2Error> {
        use diesel::connection::SimpleConnection;
        conn.batch_execute(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;",
        )
        .map_err(R2d2Error::QueryError)
    }
}

// Customizes connections for read-only access.
#[derive(Debug)]
struct ReadCustomizer;

impl CustomizeConnection<SqliteConnection, R2d2Error> for ReadCustomizer {
    fn on_acquire(&self, conn: &mut SqliteConnection) -> Result<(), R2d2Error> {
        use diesel::connection::SimpleConnection;
        conn.batch_execute("PRAGMA query_only = ON; PRAGMA busy_timeout = 5000;")
            .map_err(R2d2Error::QueryError)
    }
}

pub fn new_write_pool(db_path: &str) -> Pool {
    let manager = ConnectionManager::<SqliteConnection>::new(db_path);
    Pool::builder()
        .connection_customizer(Box::new(WriteCustomizer))
        .build(manager)
        .unwrap_or_else(|e| panic!("Failed to create write pool for {db_path}: {e}"))
}

pub fn new_read_pool(db_path: &str, max_size: u32) -> Pool {
    let manager = ConnectionManager::<SqliteConnection>::new(db_path);
    Pool::builder()
        .max_size(max_size)
        .connection_customizer(Box::new(ReadCustomizer))
        .build(manager)
        .unwrap_or_else(|e| panic!("Failed to create read pool for {db_path}: {e}"))
}
