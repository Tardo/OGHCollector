// Copyright 2025 Alexandre D. Díaz
import '@scss/pages/osv.scss';
import {initLazyTabPanes} from '@app/utils/lazy-tab';

// Odoo-version tabs beyond the active one are rendered empty by the server;
// fetch their content on first activation so a page tracking many Odoo
// versions doesn't ship every version's packages/advisories up front.
initLazyTabPanes('pills-tab');

// Hides advisories missing the filter, packages left with none, and a tab's
// table when nothing in it matches. Re-run when a lazy tab arrives, so a tab
// opened after typing comes in filtered.
const search_input = document.getElementById('osv_search');
function applyFilter() {
  const term = search_input.value.trim().toLowerCase();
  document.querySelectorAll('#osv .osv-table').forEach(table => {
    let any_visible = false;
    table.querySelectorAll('tbody.osv-package').forEach(pkg => {
      let visible = false;
      pkg.querySelectorAll('tr[data-search]').forEach(row => {
        row.hidden = term !== '' && !row.dataset.search.includes(term);
        visible ||= !row.hidden;
      });
      pkg.hidden = !visible;
      any_visible ||= visible;
    });
    const pane = table.closest('.tab-pane');
    table.closest('.panel').hidden = !any_visible;
    pane.querySelector('.osv-no-match').hidden = any_visible;
  });
}
if (search_input) {
  search_input.addEventListener('input', applyFilter);
  document
    .getElementById('osv')
    .addEventListener('lazy-tab-loaded', applyFilter);
}
