// Copyright 2026 Alexandre D. Díaz
import '@app/components/module-search';
import '@scss/pages/modules.scss';
import {initLazyTabPanes} from '@app/utils/lazy-tab';

// Delegated (not bound per `.pr-filter-group` at load) since lazy-loaded
// tabs (see initLazyTabPanes below) inject their own filter group after
// this script has already run.
document
  .getElementById('versions-tabContent')
  ?.addEventListener('click', ev => {
    const badge = ev.target.closest('.pr-filter');
    const group = badge?.closest('.pr-filter-group');
    const list = group?.closest('.tab-pane')?.querySelector('.pr-list');
    if (!list) {
      return;
    }
    const was_active = badge.classList.contains('active');
    group
      .querySelectorAll('.pr-filter')
      .forEach(b => b.classList.remove('active'));
    list.classList.remove(
      'filter-fresh',
      'filter-rotting',
      'filter-rotten',
      'filter-duplicate',
    );
    if (!was_active) {
      badge.classList.add('active');
      list.classList.add(`filter-${badge.dataset.filter}`);
    }
  });

// Odoo-version tabs beyond the active one are rendered empty by the server;
// fetch their content on first activation so a page tracking many Odoo
// versions doesn't ship every version's PRs/security findings up front.
initLazyTabPanes('versions-tab');
