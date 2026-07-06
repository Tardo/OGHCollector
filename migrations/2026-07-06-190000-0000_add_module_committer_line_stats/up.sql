-- Per-committer line-change totals (git log --shortstat insertions/deletions),
-- aggregated the same way `commits` already is. Powers the "lines written"
-- stats and the Quijote fun fact on the committer page.
ALTER TABLE module_committer ADD COLUMN insertions integer not null default 0;
ALTER TABLE module_committer ADD COLUMN deletions integer not null default 0;
