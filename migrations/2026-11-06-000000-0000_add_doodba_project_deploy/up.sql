-- What the project's `.gitlab-ci.yml` variables and `.copier-answers.yml`
-- domains say about its real instances: {gitlab_ci, prod, prod_branch,
-- prod_url, demo, demo_branch, demo_clone_prod, demo_reset_db, demo_name,
-- demo_url} (the model's `Deploy`). '{}' = not collected yet.
ALTER TABLE doodba_project ADD COLUMN deploy TEXT NOT NULL DEFAULT '{}';
