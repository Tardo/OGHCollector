// Copyright 2026 Alexandre D. Díaz
// Version migration (pages/migration.html): server-rendered, filters are
// links; this only submits the version pair on change and filters the
// module rows by text, as osv.mjs does.
import '@scss/pages/migration.scss';

const pair = document.getElementById('migration-pair');
pair?.addEventListener('change', () => pair.requestSubmit());

const search_input = document.getElementById('migration_search');
search_input?.addEventListener('input', () => {
  const term = search_input.value.trim().toLowerCase();
  let any_visible = false;
  document.querySelectorAll('#migration-modules tbody tr').forEach(row => {
    row.hidden = term !== '' && !row.dataset.search.includes(term);
    any_visible ||= !row.hidden;
  });
  const no_match = document.getElementById('migration-no-match');
  if (no_match) {
    no_match.hidden = any_visible;
  }
});
