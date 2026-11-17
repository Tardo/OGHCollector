// Copyright 2025 Alexandre D. Díaz
import {registerComponent} from 'mirlo';
import '@scss/pages/osv.scss';
import LazyTabs from '@app/components/lazy-tabs';
import {filterRows, filterTerm, matchesTerm} from '@app/utils/table-filter';

// The /osv page (pages/osv.html): Odoo-version tabs beyond the active one are
// rendered empty by the server and fetched on first activation (LazyTabs), so
// a page tracking many versions doesn't ship every version's advisories up front.
class OsvPage extends LazyTabs {
  getEventDefs() {
    const defs = super.getEventDefs();
    defs[''].events['lazy-tab-loaded'] = this.applyFilter;
    defs.osv_search = {mode: 'id', events: {input: this.applyFilter}};
    return defs;
  }

  // Hides advisories missing the filter, packages left with none, and a tab's
  // table when nothing in it matches. Re-run when a lazy tab arrives, so a tab
  // opened after typing comes in filtered.
  applyFilter() {
    const term = filterTerm(this.queryId('osv_search'));
    this.queryAll('.osv-table').forEach(table => {
      let any_visible = false;
      table.querySelectorAll('tbody.osv-package').forEach(pkg => {
        const visible = filterRows(
          pkg.querySelectorAll('tr[data-search]'),
          row => matchesTerm(row, term),
        );
        pkg.hidden = !visible;
        any_visible ||= visible;
      });
      const pane = table.closest('.tab-pane');
      table.closest('.panel').hidden = !any_visible;
      pane.querySelector('.osv-no-match').hidden = any_visible;
    });
  }
}

registerComponent('osv', OsvPage);
