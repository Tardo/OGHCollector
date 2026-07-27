-- One embedding vector per module row (module is already scoped per Odoo
-- version/repository). `embedding` is little-endian f32 bytes produced by the
-- oghembed crate's multilingual sentence-transformer; `text_hash`
-- fingerprints the embedded text (+ model tag) so the collector only
-- re-embeds modules whose description actually changed.
CREATE TABLE IF NOT EXISTS module_embedding (
    id integer primary key autoincrement,
    module_id integer not null unique,
    text_hash text not null,
    embedding blob not null,
    create_date text not null,
    update_date text not null,
    CONSTRAINT fk_module
        FOREIGN KEY (module_id)
        REFERENCES module(id)
        ON DELETE CASCADE
);
