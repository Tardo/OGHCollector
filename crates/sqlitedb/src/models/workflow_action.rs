// Copyright Alexandre D. Díaz
use diesel::prelude::*;

use crate::schema::workflow_action;
use crate::utils::date::get_sqlite_utc_now;

/// Whether the action `key` already ran (live) or was already reported
/// (dry-run), and what it gave: `Some(outcome)`.
pub fn get(conn: &mut SqliteConnection, key: &str, dry_run: bool) -> Option<Option<String>> {
    workflow_action::table
        .filter(
            workflow_action::key
                .eq(key)
                .and(workflow_action::dry_run.eq(dry_run)),
        )
        .select(workflow_action::outcome)
        .first::<Option<String>>(conn)
        .optional()
        .expect("DB error in workflow_action::get")
}

/// Records that the action `key` ran (or was reported, in dry-run).
pub fn record(
    conn: &mut SqliteConnection,
    key: &str,
    dry_run: bool,
    outcome: Option<&str>,
) -> QueryResult<usize> {
    let date = get_sqlite_utc_now();
    diesel::insert_into(workflow_action::table)
        .values((
            workflow_action::key.eq(key),
            workflow_action::dry_run.eq(dry_run),
            workflow_action::outcome.eq(outcome),
            workflow_action::date.eq(&date),
        ))
        .on_conflict((workflow_action::key, workflow_action::dry_run))
        .do_update()
        .set((
            workflow_action::outcome.eq(outcome),
            workflow_action::date.eq(&date),
        ))
        .execute(conn)
}
