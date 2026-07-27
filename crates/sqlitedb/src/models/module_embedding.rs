// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use std::collections::HashMap;

use crate::schema::module_embedding;
use crate::utils::date::get_sqlite_utc_now;

#[derive(Insertable)]
#[diesel(table_name = module_embedding)]
struct NewModuleEmbedding<'a> {
    module_id: i64,
    text_hash: &'a str,
    embedding: &'a [u8],
    create_date: &'a str,
    update_date: &'a str,
}

/// The text fields an embedding is built from, for every module row - the
/// collector re-derives the embed text from these and compares hashes to
/// decide what to (re)embed.
#[derive(QueryableByName, Debug, Clone)]
pub struct EmbedSource {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub module_id: i64,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub technical_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub name: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    pub category: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    pub description: Option<String>,
}

#[derive(QueryableByName, Debug, Clone)]
pub struct EmbeddingVectorRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub module_id: i64,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    pub embedding: Vec<u8>,
}

pub fn get_sources(conn: &mut SqliteConnection) -> Vec<EmbedSource> {
    diesel::sql_query(
        "SELECT id as module_id, technical_name, name, category, description FROM module",
    )
    .load::<EmbedSource>(conn)
    .expect("DB error in module_embedding::get_sources")
}

/// module_id -> stored text_hash, for the collector's incremental pass.
pub fn get_hashes(conn: &mut SqliteConnection) -> HashMap<i64, String> {
    module_embedding::table
        .select((module_embedding::module_id, module_embedding::text_hash))
        .load::<(i64, String)>(conn)
        .expect("DB error in module_embedding::get_hashes")
        .into_iter()
        .collect()
}

/// Every stored vector, optionally restricted to one Odoo version - the
/// brute-force candidate set for a semantic search.
// ponytail: full-table scan per query; move to sqlite-vec/an ANN index if the
// corpus ever grows past ~100k modules.
pub fn get_vectors(
    conn: &mut SqliteConnection,
    version_odoo: Option<&u8>,
) -> Vec<EmbeddingVectorRow> {
    match version_odoo {
        Some(v) => diesel::sql_query(
            "SELECT me.module_id, me.embedding FROM module_embedding as me \
             INNER JOIN module as mod ON mod.id = me.module_id \
             WHERE mod.version_odoo = ?",
        )
        .bind::<diesel::sql_types::Integer, _>(*v as i32)
        .load::<EmbeddingVectorRow>(conn)
        .expect("DB error in module_embedding::get_vectors"),
        None => diesel::sql_query("SELECT module_id, embedding FROM module_embedding")
            .load::<EmbeddingVectorRow>(conn)
            .expect("DB error in module_embedding::get_vectors"),
    }
}

#[derive(QueryableByName)]
struct IdRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    id: i64,
}

fn count_modules(conn: &mut SqliteConnection, version_odoo: Option<&u8>) -> i64 {
    #[derive(QueryableByName)]
    struct CountRow {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        n: i64,
    }
    let row: CountRow = match version_odoo {
        Some(v) => diesel::sql_query("SELECT count(*) as n FROM module WHERE version_odoo = ?")
            .bind::<diesel::sql_types::Integer, _>(*v as i32)
            .get_result(conn),
        None => diesel::sql_query("SELECT count(*) as n FROM module").get_result(conn),
    }
    .expect("DB error in module_embedding::count_modules");
    row.n
}

/// Per-module lexical score (0..1) for a free-text query: IDF-weighted share
/// of the query's tokens appearing verbatim in a module's technical name or
/// display name. This is the semantic ranking's rescue for rare proper nouns
/// (product names like "Veri*Factu" or "TicketBAI") that the embedding model
/// can't know: a token shared by few modules carries almost all the weight,
/// while ubiquitous words weigh next to nothing - IDF acts as the stopword
/// list, in any language. Two guards keep weak evidence quiet: tokens under
/// 4 chars are dropped (substring collisions like "los" inside "close"
/// defeat IDF), and tokens matching nothing (typos, words that only live in
/// descriptions) still count against the denominator at max rarity - so one
/// incidental hit among several unrecognized words stays a small boost
/// instead of hijacking the ranking.
// ponytail: technical_name+name LIKE scans only (tiny columns, one scan per
// token, max 8 tokens); bring in description via FTS5 if keyword recall on
// long texts ever matters.
pub fn lexical_scores(
    conn: &mut SqliteConnection,
    query: &str,
    version_odoo: Option<&u8>,
) -> HashMap<i64, f32> {
    let mut tokens: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 4)
        .map(|t| t.to_string())
        .collect();
    tokens.sort();
    tokens.dedup();
    tokens.truncate(8);
    let total = count_modules(conn, version_odoo) as f32;
    if tokens.is_empty() || total == 0.0 {
        return HashMap::new();
    }

    let mut weight_by_module: HashMap<i64, f32> = HashMap::new();
    let mut total_weight = 0.0f32;
    for token in &tokens {
        let pattern = format!("%{token}%");
        let rows: Vec<IdRow> = match version_odoo {
            Some(v) => diesel::sql_query(
                "SELECT id FROM module \
                 WHERE version_odoo = ? AND (technical_name LIKE ? OR name LIKE ?)",
            )
            .bind::<diesel::sql_types::Integer, _>(*v as i32)
            .bind::<diesel::sql_types::Text, _>(&pattern)
            .bind::<diesel::sql_types::Text, _>(&pattern)
            .load(conn),
            None => diesel::sql_query(
                "SELECT id FROM module WHERE technical_name LIKE ? OR name LIKE ?",
            )
            .bind::<diesel::sql_types::Text, _>(&pattern)
            .bind::<diesel::sql_types::Text, _>(&pattern)
            .load(conn),
        }
        .expect("DB error in module_embedding::lexical_scores");
        if rows.is_empty() {
            total_weight += total.ln();
            continue;
        }
        let weight = (total / rows.len() as f32).ln().max(0.0);
        total_weight += weight;
        for row in rows {
            *weight_by_module.entry(row.id).or_default() += weight;
        }
    }
    if total_weight == 0.0 {
        return HashMap::new();
    }
    weight_by_module
        .into_iter()
        .map(|(id, w)| (id, w / total_weight))
        .collect()
}

pub fn upsert(
    conn: &mut SqliteConnection,
    module_id: &i64,
    text_hash: &str,
    embedding: &[u8],
) -> QueryResult<()> {
    let now = get_sqlite_utc_now();
    diesel::insert_into(module_embedding::table)
        .values(&NewModuleEmbedding {
            module_id: *module_id,
            text_hash,
            embedding,
            create_date: &now,
            update_date: &now,
        })
        .on_conflict(module_embedding::module_id)
        .do_update()
        .set((
            module_embedding::text_hash.eq(text_hash),
            module_embedding::embedding.eq(embedding),
            module_embedding::update_date.eq(&now),
        ))
        .execute(conn)?;
    Ok(())
}

/// Drops vectors whose module row is gone. The connections don't enable
/// PRAGMA foreign_keys, so the ON DELETE CASCADE on module_id never fires -
/// module::delete_outdated leaves these behind.
pub fn delete_orphans(conn: &mut SqliteConnection) -> QueryResult<usize> {
    diesel::sql_query("DELETE FROM module_embedding WHERE module_id NOT IN (SELECT id FROM module)")
        .execute(conn)
}
