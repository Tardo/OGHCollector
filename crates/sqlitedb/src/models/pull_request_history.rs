// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::pull_request_history;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = pull_request_history, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub module_technical_name: String,
    pub version_odoo: i32,
    pub gh_repository_id: i64,
    pub prid: i64,
    pub created_at: String,
    pub closed_at: String,
}

#[derive(Insertable)]
#[diesel(table_name = pull_request_history)]
struct NewPullRequestHistory<'a> {
    module_technical_name: &'a str,
    version_odoo: i32,
    gh_repository_id: i64,
    prid: i64,
    created_at: &'a str,
    closed_at: &'a str,
}

/// Records a migration PR/MR that was merged, called from
/// `pull_request::delete_outdated` right before the live row is removed - only
/// for PRs the caller has confirmed were merged (closed-without-merge PRs are
/// dropped there, never reaching this table).
/// `closed_at` is the collector's detection time, not the provider's real
/// merge timestamp - see the migration's comment for why.
#[allow(clippy::too_many_arguments)]
pub fn add(
    conn: &mut SqliteConnection,
    module_technical_name: &str,
    version_odoo: i32,
    gh_repository_id: i64,
    prid: i64,
    created_at: &str,
    closed_at: &str,
) -> QueryResult<Model> {
    diesel::insert_into(pull_request_history::table)
        .values(NewPullRequestHistory {
            module_technical_name,
            version_odoo,
            gh_repository_id,
            prid,
            created_at,
            closed_at,
        })
        .execute(conn)?;

    Ok(Model {
        id: crate::models::last_insert_rowid(conn),
        module_technical_name: module_technical_name.to_string(),
        version_odoo,
        gh_repository_id,
        prid,
        created_at: created_at.to_string(),
        closed_at: closed_at.to_string(),
    })
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct AcceptanceStatsInfo {
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub version_odoo: i32,
    #[diesel(sql_type = diesel::sql_types::Double)]
    pub avg_days: f64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub closed_count: i64,
}

/// Average days a migration PR/MR stayed open before being merged, per Odoo
/// version - the closest proxy for "acceptance time" this table can give
/// (see module doc: `closed_at` is a detection time, not the provider's exact
/// merge timestamp). Rows are merged-only, so `closed_count` here means
/// merged PRs, not every closure.
pub fn average_days_open_by_version(conn: &mut SqliteConnection) -> Vec<AcceptanceStatsInfo> {
    diesel::sql_query(
        "SELECT version_odoo, \
         AVG(julianday(closed_at) - julianday(created_at)) as avg_days, \
         COUNT(*) as closed_count \
         FROM pull_request_history \
         GROUP BY version_odoo",
    )
    .load::<AcceptanceStatsInfo>(conn)
    .expect("DB error in pull_request_history::average_days_open_by_version")
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::sqlite::SqliteConnection;
    use diesel::Connection;
    use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};

    const MIGRATIONS: EmbeddedMigrations = embed_migrations!("../../migrations");

    fn setup_db() -> SqliteConnection {
        let mut conn = SqliteConnection::establish(":memory:").unwrap();
        conn.run_pending_migrations(MIGRATIONS).unwrap();
        conn
    }

    #[test]
    fn test_average_days_open_by_version() {
        let mut conn = setup_db();
        let org = crate::models::gh_organization::add(&mut conn, "HistOrg").unwrap();
        let repo = crate::models::gh_repository::add(&mut conn, &org.id, "hist-repo").unwrap();

        add(
            &mut conn,
            "mod_a",
            16,
            repo.id,
            1,
            "2024-01-01 00:00:00",
            "2024-01-03 00:00:00",
        )
        .unwrap();
        add(
            &mut conn,
            "mod_b",
            16,
            repo.id,
            2,
            "2024-01-01 00:00:00",
            "2024-01-05 00:00:00",
        )
        .unwrap();
        add(
            &mut conn,
            "mod_c",
            17,
            repo.id,
            3,
            "2024-01-01 00:00:00",
            "2024-01-11 00:00:00",
        )
        .unwrap();

        let mut stats = average_days_open_by_version(&mut conn);
        stats.sort_by_key(|s| s.version_odoo);

        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].version_odoo, 16);
        assert_eq!(stats[0].closed_count, 2);
        assert!((stats[0].avg_days - 3.0).abs() < 0.01);
        assert_eq!(stats[1].version_odoo, 17);
        assert_eq!(stats[1].closed_count, 1);
        assert!((stats[1].avg_days - 10.0).abs() < 0.01);
    }
}
