// Copyright 2026 Alexandre D. Díaz
// Doodba projects (pages/projects.html, pages/project.html): server-rendered;
// this only filters the table rows by text, as migration.mjs does.
import '@scss/pages/projects.scss';

const search_input = document.getElementById('projects_search');
search_input?.addEventListener('input', () => {
  const term = search_input.value.trim().toLowerCase();
  let any_visible = false;
  document.querySelectorAll('#projects-table tbody tr').forEach(row => {
    row.hidden = term !== '' && !row.dataset.search.includes(term);
    any_visible ||= !row.hidden;
  });
  const no_match = document.getElementById('projects-no-match');
  if (no_match) {
    no_match.hidden = any_visible;
  }
});
