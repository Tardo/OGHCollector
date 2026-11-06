// Copyright 2026 Alexandre D. Díaz
// Doodba projects (pages/projects.html, pages/project.html): server-rendered;
// this only filters the table rows by text, as migration.mjs does.
import '@scss/pages/projects.scss';

const search_input = document.getElementById('projects_search');
const version_pills = document.getElementById('projects-versions');
let version = '';
const flags = new Set();

function apply_filter() {
  const term = search_input ? search_input.value.trim().toLowerCase() : '';
  let any_visible = false;
  document.querySelectorAll('#projects-table tbody tr').forEach(row => {
    row.hidden =
      (term !== '' && !row.dataset.search.includes(term)) ||
      (version !== '' && row.dataset.version !== version) ||
      [...flags].some(f => !row.dataset.flags.split(' ').includes(f));
    any_visible ||= !row.hidden;
  });
  const no_match = document.getElementById('projects-no-match');
  if (no_match) {
    no_match.hidden = any_visible;
  }
}

search_input?.addEventListener('input', apply_filter);
version_pills?.addEventListener('click', e => {
  const btn = e.target.closest('button[data-version]');
  if (!btn) {
    return;
  }
  version = btn.dataset.version;
  version_pills
    .querySelectorAll('.nav-link')
    .forEach(b => b.classList.toggle('active', b === btn));
  apply_filter();
});

document.getElementById('projects-flags')?.addEventListener('click', e => {
  const btn = e.target.closest('button[data-flag]');
  if (!btn) {
    return;
  }
  const on = !flags.has(btn.dataset.flag);
  flags[on ? 'add' : 'delete'](btn.dataset.flag);
  btn.classList.toggle('active', on);
  btn.setAttribute('aria-pressed', String(on));
  apply_filter();
});

// Click a sortable header: numeric columns descending first, text ascending.
document.querySelectorAll('#projects-table th[data-sort]').forEach(th => {
  th.style.cursor = 'pointer';
  th.addEventListener('click', () => {
    const key = th.dataset.sort;
    const text = key === 'name' || key === 'version' || key === 'last';
    const dir = th.dataset.dir === 'asc' ? -1 : 1;
    th.parentElement.querySelectorAll('th').forEach(o => delete o.dataset.dir);
    th.dataset.dir = dir === 1 ? 'asc' : 'desc';
    const body = document.querySelector('#projects-table tbody');
    const rows = [...body.querySelectorAll('tr')];
    rows.sort((a, b) => {
      const x = a.dataset[key];
      const y = b.dataset[key];
      const c = text
        ? x.localeCompare(y, undefined, {numeric: true})
        : Number(x) - Number(y);
      return (text ? c : -c) * dir;
    });
    rows.forEach(r => body.appendChild(r));
  });
});
