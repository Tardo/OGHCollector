// Copyright 2026 Alexandre D. Díaz
import '@app/components/module-search';
import '@scss/pages/modules.scss';

function initSortableTables() {
  document.querySelectorAll('table.sortable-table').forEach(table => {
    const headers = [...table.tHead.rows[0].cells];
    headers.forEach(th => {
      if (!th.dataset.sortKey) return;
      th.addEventListener('click', () => {
        const asc = th.dataset.sortDir !== 'asc';
        headers.forEach(h => delete h.dataset.sortDir);
        th.dataset.sortDir = asc ? 'asc' : 'desc';

        const idx = headers.indexOf(th);
        const rows = [...table.tBodies[0].rows];
        rows.sort((a, b) => {
          const diff =
            parseFloat(a.cells[idx].dataset.sort) -
            parseFloat(b.cells[idx].dataset.sort);
          return asc ? diff : -diff;
        });
        rows.forEach(row => table.tBodies[0].appendChild(row));
      });
    });
  });
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', initSortableTables);
} else {
  initSortableTables();
}
