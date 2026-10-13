ALTER TABLE pull_request ADD COLUMN module_technical_name text not null default '';
UPDATE pull_request SET module_technical_name = coalesce(
    (SELECT module_technical_name FROM pull_request_module pm WHERE pm.pull_request_id = pull_request.id ORDER BY pm.is_new DESC LIMIT 1), '');
DROP TABLE IF EXISTS pull_request_module;
DROP INDEX IF EXISTS idx_pull_request_head_sha;
ALTER TABLE pull_request DROP COLUMN analysis_version;
ALTER TABLE pull_request DROP COLUMN analyzed_sha;
ALTER TABLE pull_request DROP COLUMN labels;
ALTER TABLE pull_request DROP COLUMN is_draft;
ALTER TABLE pull_request DROP COLUMN author;
ALTER TABLE pull_request DROP COLUMN head_sha;
