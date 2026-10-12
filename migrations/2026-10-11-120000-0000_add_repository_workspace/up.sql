-- Workspaces (areas) a repository is shown in, from collector.yaml's
-- `workspaces:`. A repository without rows is in every workspace (the
-- default). Rewritten for each repo by every collector job that passes
-- workspaces (`oghcollector` with no args always does).
CREATE TABLE IF NOT EXISTS gh_repository_workspace (
    gh_repository_id INTEGER NOT NULL REFERENCES gh_repository(id) ON DELETE CASCADE,
    workspace TEXT NOT NULL,
    PRIMARY KEY (gh_repository_id, workspace)
);
