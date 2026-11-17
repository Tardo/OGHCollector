// Copyright 2026 Alexandre D. Díaz
// Both localization pages: the globe index (/localization) and one
// country's results (/localization/{code}). Wraps the server's markup.
import {Component, registerComponent} from 'mirlo';
import {whereAlpha2} from 'iso-3166-1';
import {t} from '@app/utils/i18n';

class Localization extends Component {
  onSetup() {
    Component.disableShadow();
  }

  onStart() {
    super.onStart();
    // The world atlas is heavy: only the index page loads it.
    if (this.query('#localization-globe')) {
      import('@app/components/localization-globe');
    }
    const page = this.query('.localization-results');
    const country =
      page &&
      whereAlpha2(page.dataset.country === 'UK' ? 'GB' : page.dataset.country);
    if (country) {
      this.queryId('localization-country-name').textContent = country.country;
      document.title = t(
        'Localization: {0} - Odoo Module Metadata',
        country.country,
      );
    }
  }
}

registerComponent('localization', Localization);
