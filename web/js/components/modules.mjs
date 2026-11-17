// Copyright 2026 Alexandre D. Díaz
import {registerComponent} from 'mirlo';
import '@app/components/module-search';
import '@scss/pages/modules.scss';
import LazyTabs from '@app/components/lazy-tabs';

// The /modules page (pages/modules.html): lazy Odoo-version tabs plus the
// pull-request filter badges. Delegated on the host, since lazy-loaded tabs
// inject their own filter group after start.
class ModulesPage extends LazyTabs {
  getEventDefs() {
    const defs = super.getEventDefs();
    defs[''].events.click = this.onClick;
    return defs;
  }

  // `/modules#modver-19.0` (links from the Overview's version table) opens
  // that version's tab. On `load`, so Bootstrap's tab handler (main bundle) is in.
  onStart() {
    super.onStart();
    window.addEventListener('load', () => {
      document.getElementById(`${location.hash.slice(1)}-tab`)?.click();
    });
  }

  onClick(ev) {
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
  }
}

registerComponent('modules', ModulesPage);
