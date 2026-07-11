// Copyright Alexandre D. Díaz
import '@app/components/module-search';
import '@scss/pages/module.scss';
import {fetchIntoPane, initLazyTabPanes} from '@app/utils/lazy-tab';

document.body.addEventListener('keydown', ev => {
  if (ev.ctrlKey || ev.altKey || ev.metaKey) {
    return;
  }
  const search_modal = document.getElementById('module_search');
  if (search_modal.classList.contains('d-none')) {
    if (ev.key.length !== 1) {
      return;
    }
    search_modal.classList.remove('d-none');
    const search_comp = search_modal.querySelector('mirlo-module-search');
    const input = search_comp.query('input');
    input.focus();
  } else if (ev.code === 'Escape') {
    search_modal.classList.add('d-none');
    const search_comp = search_modal.querySelector('mirlo-module-search');
    const input = search_comp.query('input');
    input.value = '';
  }
});

// Delegated (not queried once at load) since lazy-loaded tabs inject their
// own `.module-version-select` after this script has already run. Re-fetches
// just this pane instead of reloading the page - a full-page reload has
// nowhere to send the selection: only the active Odoo-version tab is
// server-rendered (see pages/module.html), and there's no tab activated
// from the URL, so a reload on any other tab would silently drop which
// version was picked.
document
  .getElementById('versions-tabContent')
  ?.addEventListener('change', ev => {
    const select = ev.target.closest('.module-version-select');
    const pane = select?.closest('.tab-pane');
    if (!pane) {
      return;
    }
    const url = `${window.location.pathname}/tab/${encodeURIComponent(select.dataset.odooVersion)}?version=${encodeURIComponent(select.value)}`;
    fetchIntoPane(pane, url);
  });

// Odoo-version tabs beyond the active one are rendered empty by the server;
// fetch their content on first activation so a module tracked across many
// versions doesn't ship every version's full code analysis on initial load.
initLazyTabPanes('versions-tab');
