// Copyright Alexandre D. Díaz
//! Standalone migration runner, called directly from `docker-entrypoint.sh` before
//! `server`/`mcp`/`collector` start, so the schema is current no matter which binary
//! is the container's actual entrypoint (previously only `collector` ever migrated).
use named_lock::NamedLock;
use std::fs::{self, File};
use std::path::Path;

fn main() {
    let db_path = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("OGHCOLLECTOR_DB_PATH").ok())
        .unwrap_or_else(|| "data/data.db".to_string());

    if let Some(parent) = Path::new(&db_path).parent() {
        fs::create_dir_all(parent).expect("Can't create DB directory");
    }
    if !Path::new(&db_path).exists() {
        File::create(&db_path).expect("Can't create DB file");
    }

    // `server`/`mcp`/`collector` containers share the same DB volume and can start
    // concurrently (see docker-compose.yaml), so serialize migrations across processes
    // with a lock file next to the DB rather than relying on SQLite's busy_timeout alone.
    let lock_path = format!("{db_path}.migrate.lock");
    let lock = NamedLock::with_path(&lock_path).expect("Can't create migration lock");
    let _guard = lock.lock().expect("Can't acquire migration lock");

    let pool = sqlitedb::new_write_pool(&db_path);
    let mut conn = pool.get().expect("Can't get DB connection");
    sqlitedb::run_migrations(&mut conn).expect("Can't run migrations");
    println!("Database '{db_path}' schema is up to date.");
}
