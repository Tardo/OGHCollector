import {whereAlpha2} from 'iso-3166-1';

const page = document.querySelector('.localization-results');
const country = whereAlpha2(
  page.dataset.country === 'UK' ? 'GB' : page.dataset.country,
);
if (country) {
  document.querySelector('#localization-country-name').textContent =
    country.country;
  document.title = `Localization: ${country.country} - Odoo Module Metadata`;
}
