// Copyright 2026 Alexandre D. Díaz
// Both localization pages: the globe index (/localization) and one
// country's results (/localization/{code}).
import {whereAlpha2} from 'iso-3166-1';

// The world atlas is heavy: only the index page loads it.
if (document.querySelector('#localization-globe')) {
  import('@app/components/localization-globe');
}

const page = document.querySelector('.localization-results');
const country =
  page &&
  whereAlpha2(page.dataset.country === 'UK' ? 'GB' : page.dataset.country);
if (country) {
  document.querySelector('#localization-country-name').textContent =
    country.country;
  document.title = `Localization: ${country.country} - Odoo Module Metadata`;
}
