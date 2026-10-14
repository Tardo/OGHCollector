-- What workflows did outside the collector (or would have done, in dry-run):
-- an action runs once per `key` (workflow, node and the item it acted on), so
-- re-runs and webhooks never repeat it. `outcome` is its result: a PR URL, or
-- an AI answer, reused instead of asking again.
CREATE TABLE IF NOT EXISTS workflow_action (
    id integer primary key autoincrement,
    key text not null,
    dry_run boolean not null,
    outcome text,
    date text not null
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_workflow_action ON workflow_action(key, dry_run);
