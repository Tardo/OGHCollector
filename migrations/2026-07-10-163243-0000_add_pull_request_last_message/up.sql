-- Providers already return this for free in the same list response used to
-- fetch open PRs/MRs (GitHub `updated_at`, GitLab `updated_at`), so it's used
-- as-is as a "last activity" proxy for the last message date - no per-PR
-- comments-API call needed. Nullable like `created_at`: unknown for rows
-- inserted before this migration until the collector's next run refreshes them.
ALTER TABLE pull_request ADD COLUMN last_message_at text;
