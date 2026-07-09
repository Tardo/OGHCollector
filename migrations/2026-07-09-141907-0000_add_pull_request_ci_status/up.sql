-- Surfaces PR age and CI status in the PR lists (modules page, module page,
-- doodba migration plan). Both nullable: `created_at` is unknown for rows
-- inserted before this migration until the collector's next run refreshes
-- them, and `ci_status` ('success'/'pending'/'failure') is unknown when the
-- provider reports no checks at all for the head commit.
ALTER TABLE pull_request ADD COLUMN created_at text;
ALTER TABLE pull_request ADD COLUMN ci_status text;
