// Copyright Alexandre D. Díaz
use diesel::prelude::*;

use crate::schema::gh_repository_workspace;

/// Replaces the workspaces a repository is shown in; empty = every workspace.
pub fn set(
    conn: &mut SqliteConnection,
    gh_repository_id: &i64,
    workspaces: &[String],
) -> QueryResult<()> {
    use gh_repository_workspace::dsl;
    conn.transaction(|conn| {
        diesel::delete(
            dsl::gh_repository_workspace.filter(dsl::gh_repository_id.eq(gh_repository_id)),
        )
        .execute(conn)?;
        for ws in workspaces {
            diesel::insert_into(dsl::gh_repository_workspace)
                .values((
                    dsl::gh_repository_id.eq(gh_repository_id),
                    dsl::workspace.eq(ws),
                ))
                .execute(conn)?;
        }
        Ok(())
    })
}

/// Every workspace at least one repository is restricted to, sorted.
pub fn names(conn: &mut SqliteConnection) -> Vec<String> {
    use gh_repository_workspace::dsl;
    dsl::gh_repository_workspace
        .select(dsl::workspace)
        .distinct()
        .order(dsl::workspace)
        .load(conn)
        .expect("DB error in gh_repository_workspace::names")
}
