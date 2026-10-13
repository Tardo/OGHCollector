-- Every open PR/MR against a collected version branch is tracked now, not only
-- OCA-named migration branches, and one PR can touch several modules: the
-- module moves to `pull_request_module`, one row per module a PR changes.
-- `is_new` = the module doesn't exist on the base branch (a migration/port or a
-- brand-new module), which is what "migration PR" means everywhere else; the
-- rows tracked so far were all migration PRs.
ALTER TABLE pull_request ADD COLUMN head_sha text;
ALTER TABLE pull_request ADD COLUMN author text;
ALTER TABLE pull_request ADD COLUMN is_draft boolean not null default 0;
-- JSON array of label names.
ALTER TABLE pull_request ADD COLUMN labels text;
-- Head commit (and collector::analyzer::ANALYSIS_VERSION) the modules were
-- last analyzed at; re-analyzed when either differs.
ALTER TABLE pull_request ADD COLUMN analyzed_sha text;
ALTER TABLE pull_request ADD COLUMN analysis_version integer not null default 0;

CREATE TABLE IF NOT EXISTS pull_request_module (
    id integer primary key autoincrement,
    pull_request_id integer not null references pull_request(id) on delete cascade,
    module_technical_name text not null,
    is_new boolean not null default 0,
    -- JSON (manifest, security findings, migration notes) of the module at the
    -- PR head; NULL when the PR changes no code of it (translations, docs).
    analysis text
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_pull_request_module ON pull_request_module(pull_request_id, module_technical_name);
CREATE INDEX IF NOT EXISTS idx_pull_request_module_name ON pull_request_module(module_technical_name);
CREATE INDEX IF NOT EXISTS idx_pull_request_head_sha ON pull_request(head_sha);

INSERT INTO pull_request_module (pull_request_id, module_technical_name, is_new)
    SELECT id, module_technical_name, 1 FROM pull_request;
ALTER TABLE pull_request DROP COLUMN module_technical_name;
