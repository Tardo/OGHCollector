// Copyright Alexandre D. Díaz
#[cfg(feature = "fixture")]
pub mod fixture;
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

// Customizes connections for write access (timeouts; WAL is set once by
// `new_write_pool`).
#[derive(Debug)]
struct WriteCustomizer;

impl CustomizeConnection<SqliteConnection, R2d2Error> for WriteCustomizer {
    fn on_acquire(&self, conn: &mut SqliteConnection) -> Result<(), R2d2Error> {
        use diesel::connection::SimpleConnection;
        conn.batch_execute("PRAGMA busy_timeout = 5000; PRAGMA synchronous = NORMAL;")
            .map_err(R2d2Error::QueryError)
    }
}

// Customizes connections for read-only access, optionally scoped to one
// workspace (see `new_workspace_read_pool`).
#[derive(Debug)]
struct ReadCustomizer(Option<String>);

impl CustomizeConnection<SqliteConnection, R2d2Error> for ReadCustomizer {
    fn on_acquire(&self, conn: &mut SqliteConnection) -> Result<(), R2d2Error> {
        use diesel::connection::SimpleConnection;
        conn.batch_execute("PRAGMA busy_timeout = 5000;")
            .map_err(R2d2Error::QueryError)?;
        // TEMP objects can't be created once query_only is on.
        if let Some(ws) = &self.0 {
            conn.batch_execute(&workspace_views(ws))
                .map_err(R2d2Error::QueryError)?;
        }
        conn.batch_execute("PRAGMA query_only = ON;")
            .map_err(R2d2Error::QueryError)
    }
}

/// TEMP views that shadow `gh_repository`, `gh_organization` and `module`
/// with only what `ws` shows: SQLite resolves an unqualified table name in
/// `temp` before `main`, so every existing query (raw SQL or Diesel DSL, which
/// never qualifies names) on such a connection sees the workspace alone.
/// Queries that must see everything (dependency closures across workspaces)
/// use the plain read pool instead. A repository without
/// `gh_repository_workspace` rows is in every workspace.
fn workspace_views(ws: &str) -> String {
    let ws = ws.replace('\'', "''");
    format!(
        "CREATE TEMP VIEW gh_repository AS SELECT * FROM main.gh_repository AS r \
         WHERE NOT EXISTS (SELECT 1 FROM main.gh_repository_workspace AS w \
                           WHERE w.gh_repository_id = r.id) \
            OR EXISTS (SELECT 1 FROM main.gh_repository_workspace AS w \
                       WHERE w.gh_repository_id = r.id AND w.workspace = '{ws}'); \
         CREATE TEMP VIEW gh_organization AS SELECT * FROM main.gh_organization \
         WHERE id IN (SELECT gh_organization_id FROM temp.gh_repository); \
         CREATE TEMP VIEW module AS SELECT * FROM main.module \
         WHERE gh_repository_id IN (SELECT id FROM temp.gh_repository);"
    )
}

pub fn new_write_pool(db_path: &str) -> Pool {
    // WAL is persistent, so it's switched on once here, on a lone connection.
    // Done per pooled connection (r2d2 opens them all at once), every switch
    // but one hit "database is locked" on a new DB: SQLite fails a shared →
    // exclusive lock upgrade at once, busy_timeout or not, to avoid deadlock.
    {
        use diesel::connection::SimpleConnection;
        use diesel::Connection;
        let mut conn = SqliteConnection::establish(db_path)
            .unwrap_or_else(|e| panic!("Failed to open {db_path}: {e}"));
        conn.batch_execute("PRAGMA busy_timeout = 5000; PRAGMA journal_mode = WAL;")
            .unwrap_or_else(|e| panic!("Failed to enable WAL on {db_path}: {e}"));
    }
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
        .connection_customizer(Box::new(ReadCustomizer(None)))
        .build(manager)
        .unwrap_or_else(|e| panic!("Failed to create read pool for {db_path}: {e}"))
}

/// Read pool whose connections only see the repositories (and their
/// organizations and modules) of workspace `ws`: see `workspace_views`.
pub fn new_workspace_read_pool(db_path: &str, max_size: u32, ws: &str) -> Pool {
    let manager = ConnectionManager::<SqliteConnection>::new(db_path);
    Pool::builder()
        .max_size(max_size)
        .connection_customizer(Box::new(ReadCustomizer(Some(ws.to_string()))))
        .build(manager)
        .unwrap_or_else(|e| panic!("Failed to create read pool for {db_path} ({ws}): {e}"))
}

// Like `new_read_pool` but returns `None` instead of panicking when the DB is
// missing or unreadable. Callers that must degrade gracefully (e.g. the scanner
// inferring a version from collector data) use this so a missing DB never
// crashes the probe.
pub fn try_read_pool(db_path: &str, max_size: u32) -> Option<Pool> {
    let manager = ConnectionManager::<SqliteConnection>::new(db_path);
    Pool::builder()
        .max_size(max_size)
        .connection_customizer(Box::new(ReadCustomizer(None)))
        .build(manager)
        .ok()
}

#[cfg(test)]
mod tests {
    use crate::models::{dependency, dependency_module, gh_organization, gh_repository};
    use crate::models::{gh_repository_workspace, module};
    use diesel::connection::SimpleConnection;

    // Repos a (workspace x), b (y) and c (none = every workspace); a's module
    // depends on b's.
    #[test]
    fn workspace_pool_hides_other_workspaces_only() {
        let path = std::env::temp_dir().join(format!("ogh-ws-{}.db", std::process::id()));
        let db = path.to_str().unwrap();
        let _ = std::fs::remove_file(db);
        let write = super::new_write_pool(db);
        let mut conn = write.get().unwrap();
        super::run_migrations(&mut conn).unwrap();
        let org = gh_organization::add(&mut conn, "org").unwrap();
        for (repo, ws) in [("a", vec!["x"]), ("b", vec!["y"]), ("c", vec![])] {
            let repo = gh_repository::add(&mut conn, &org.id, repo).unwrap();
            let ws: Vec<String> = ws.into_iter().map(String::from).collect();
            gh_repository_workspace::set(&mut conn, &repo.id, &ws).unwrap();
            conn.batch_execute(&format!(
                "INSERT INTO module (technical_name, version_odoo, name, version_module, \
                 auto_install, application, installable, gh_repository_id, create_date, \
                 update_date, folder_size, last_commit_hash, last_commit_author, \
                 last_commit_name, last_commit_date, source_path, analysis_version) \
                 VALUES ('mod_{0}', 180, '{0}', '1.0', 0, 0, 1, {1}, '', '', 0, '', '', '', '', \
                 '', 0)",
                repo.name, repo.id
            ))
            .unwrap();
        }
        let mod_a = module::get_by_technical_name_odoo_version(&mut conn, &["mod_a".into()], &180)
            .remove(0);
        dependency_module::add(&mut conn, &1, "mod_b", &mod_a.id).unwrap();
        assert_eq!(gh_repository_workspace::names(&mut conn), ["x", "y"]);

        let names = |pool: &super::Pool| -> Vec<String> {
            let mut names: Vec<String> = module::list(&mut pool.get().unwrap())
                .into_iter()
                .map(|m| m.technical_name)
                .collect();
            names.sort();
            names
        };
        let x = super::new_workspace_read_pool(db, 1, "x");
        assert_eq!(names(&x), ["mod_a", "mod_c"]);
        let y = super::new_workspace_read_pool(db, 1, "y");
        assert_eq!(names(&y), ["mod_b", "mod_c"]);
        let global = super::new_read_pool(db, 1);
        assert_eq!(names(&global), ["mod_a", "mod_b", "mod_c"]);
        // The plain pool (module pages, closures) still resolves the dependency
        // on the other workspace's module.
        let deps = dependency::get_full_dependency_info(&mut global.get().unwrap(), &mod_a);
        assert!(deps.nodes.values().any(|n| n.technical_name == "mod_b"));

        // Views are live: membership changes show without new connections.
        gh_repository_workspace::set(&mut conn, &mod_a.gh_repository_id, &[]).unwrap();
        assert_eq!(names(&y), ["mod_a", "mod_b", "mod_c"]);
        drop(conn);
        let _ = std::fs::remove_file(db);
    }
}
