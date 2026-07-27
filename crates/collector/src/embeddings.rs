// Copyright Alexandre D. Díaz
use sqlitedb::models;
use sqlitedb::DbSqliteConnection as SqliteConnection;

/// Builds the text a module's embedding is generated from. The model only
/// looks at the first ~128 tokens, so lead with the strongest signal
/// (name/category) and cut the description early.
// ponytail: long descriptions are only embedded by their opening - chunk +
// mean-pool per module if recall on long docs ever matters.
fn build_embed_text(src: &models::module_embedding::EmbedSource) -> String {
    let desc = src
        .description
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut text = format!(
        "{} ({}). Category: {}. {}",
        src.name,
        src.technical_name,
        src.category.as_deref().unwrap_or("Uncategorized"),
        desc
    );
    if let Some((idx, _)) = text.char_indices().nth(1500) {
        text.truncate(idx);
    }
    text
}

/// Global incremental pass, independent of which org/version this run
/// collected: (re)embeds every module whose embed text changed or that has no
/// vector yet (so the first run after the migration backfills the whole DB),
/// and drops vectors of deleted modules. A model download/load failure only
/// logs - a collector run must not die because HuggingFace is unreachable.
pub fn update_embeddings(conn: &mut SqliteConnection) {
    let stored = models::module_embedding::get_hashes(conn);
    let pending: Vec<(i64, String, String)> = models::module_embedding::get_sources(conn)
        .iter()
        .filter_map(|src| {
            let text = build_embed_text(src);
            let hash = oghembed::text_hash(&text);
            (stored.get(&src.module_id) != Some(&hash)).then_some((src.module_id, hash, text))
        })
        .collect();
    let _ = models::module_embedding::delete_orphans(conn);
    if pending.is_empty() {
        return;
    }
    log::info!("Generating embeddings for '{}' modules...", pending.len());
    let texts: Vec<&str> = pending.iter().map(|(_, _, text)| text.as_str()).collect();
    let vectors = match oghembed::embed_texts(&texts) {
        Ok(vectors) => vectors,
        Err(err) => {
            log::warn!("Can't generate embeddings: {err}. Skipping...");
            return;
        }
    };
    for ((module_id, hash, _), vector) in pending.iter().zip(vectors) {
        models::module_embedding::upsert(conn, module_id, hash, &oghembed::vec_to_bytes(&vector))
            .unwrap();
    }
}
