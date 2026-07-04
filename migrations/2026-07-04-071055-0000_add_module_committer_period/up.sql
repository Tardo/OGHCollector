-- Per-(year, month) commit breakdown per committer per module, so the committers
-- page can rank by "this month"/"this year" in addition to the existing all-time
-- total in module_committer. Populated by the collector alongside module_committer.
CREATE TABLE IF NOT EXISTS module_committer_period (
    id integer primary key autoincrement,
    module_id integer not null references module(id),
    committer_id integer not null references committer(id),
    year integer not null,
    month integer not null,
    commits integer not null,
    CONSTRAINT fk_module
        FOREIGN KEY (module_id)
        REFERENCES module(id)
        ON DELETE CASCADE,
    CONSTRAINT fk_committer
        FOREIGN KEY (committer_id)
        REFERENCES committer(id)
        ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_module_committer_period ON module_committer_period(module_id, committer_id, year, month);
CREATE INDEX IF NOT EXISTS idx_module_committer_period_year_month ON module_committer_period(year, month);
