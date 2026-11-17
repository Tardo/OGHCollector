// Copyright 2026 Alexandre D. Díaz
// Version migration (pages/migration.html): server-rendered, filters are
// links; this only submits the version pair on change and filters the
// module rows by text.
import {Component, registerComponent} from 'mirlo';
import '@scss/pages/migration.scss';
import {filterRows, filterTerm, matchesTerm} from '@app/utils/table-filter';

class Migration extends Component {
  onSetup() {
    Component.disableShadow();
    Component.useEvents({
      'migration-pair': {mode: 'id', events: {change: this.onChangePair}},
      migration_search: {mode: 'id', events: {input: this.onInputSearch}},
    });
  }

  onChangePair(ev) {
    ev.currentTarget.requestSubmit();
  }

  onInputSearch() {
    const term = filterTerm(this.queryId('migration_search'));
    const any_visible = filterRows(
      this.queryAll('#migration-modules tbody tr'),
      row => matchesTerm(row, term),
    );
    const no_match = this.queryId('migration-no-match');
    if (no_match) {
      no_match.hidden = any_visible;
    }
  }
}

registerComponent('migration', Migration);
