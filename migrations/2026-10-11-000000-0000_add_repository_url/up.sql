-- Clone URL of the repository as the collector saw it (GitHub or any GitLab
-- host). NULL = collected before this column existed: links fall back to
-- github.com/{org}/{name} until the next collector run fills it.
ALTER TABLE gh_repository ADD COLUMN url TEXT;
