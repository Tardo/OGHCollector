// Copyright 2025 Alexandre D. Díaz
import '@scss/pages/osv.scss';
import {initLazyTabPanes} from '@app/utils/lazy-tab';

// Odoo-version tabs beyond the active one are rendered empty by the server;
// fetch their content on first activation so a page tracking many Odoo
// versions doesn't ship every version's packages/vulnerabilities up front.
initLazyTabPanes('pills-tab');

const search_input = document.getElementById('osv_search');
if (search_input) {
  search_input.addEventListener('input', () => {
    const term = search_input.value.trim().toLowerCase();
    document.querySelectorAll('#osv table').forEach(table => {
      let visible_rows = 0;
      table.querySelectorAll('tbody tr[data-search]').forEach(row => {
        const match = term === '' || row.dataset.search.includes(term);
        row.classList.toggle('d-none', !match);
        if (match) {
          visible_rows += 1;
        }
      });
      table.classList.toggle('d-none', visible_rows === 0);
    });
  });
}
