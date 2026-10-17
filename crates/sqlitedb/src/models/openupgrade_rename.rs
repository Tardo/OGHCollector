// Copyright Alexandre D. Díaz
//! Module renames/merges OpenUpgrade records per Odoo version (the
//! collector's `openupgrade.rs` fills it). Not filtered by workspace: it's
//! upstream knowledge, joined by module name. Read whole (`all`) by
//! `/migration` and the module page header, which follow renames across
//! versions.
use diesel::prelude::*;
use serde::Serialize;

use crate::schema::openupgrade_rename;

#[derive(Queryable, Selectable, Debug, Clone, Serialize)]
#[diesel(table_name = openupgrade_rename)]
pub struct Model {
    pub version_odoo: i32,
    pub old_name: String,
    pub new_name: String,
    pub merged: bool,
}

/// Replaces what's recorded for `version_odoo` with `renames` (old name,
/// new name, merged into an existing module).
pub fn replace_for_version(
    conn: &mut SqliteConnection,
    version_odoo: i32,
    renames: &[(String, String, bool)],
) -> QueryResult<()> {
    use openupgrade_rename::dsl;
    conn.transaction(|conn| {
        diesel::delete(dsl::openupgrade_rename.filter(dsl::version_odoo.eq(version_odoo)))
            .execute(conn)?;
        for (old_name, new_name, merged) in renames {
            diesel::insert_or_ignore_into(dsl::openupgrade_rename)
                .values((
                    dsl::version_odoo.eq(version_odoo),
                    dsl::old_name.eq(old_name),
                    dsl::new_name.eq(new_name),
                    dsl::merged.eq(merged),
                ))
                .execute(conn)?;
        }
        Ok(())
    })
}

pub fn all(conn: &mut SqliteConnection) -> Vec<Model> {
    openupgrade_rename::table
        .select(Model::as_select())
        .load(conn)
        .expect("DB error in openupgrade_rename::all")
}
