// Copyright Alexandre D. Díaz
// UI texts built in JS, translated by i18next. The catalog is the server's gettext one
// (web/templates/i18n/{lang}.po, entries referenced from `web/js`), embedded by the layout as
// <script id="i18n-data">; the English text is the key, a missing key shows it as is.
// See crates/server/src/i18n.rs. `{0}`, `{1}`... take `args`.
import i18next from 'i18next';

function readCatalog() {
  try {
    return JSON.parse(document.getElementById('i18n-data').textContent);
  } catch {
    return {};
  }
}

const lang = document.documentElement.lang || 'en';
i18next.init({
  lng: lang,
  fallbackLng: false,
  resources: {[lang]: {translation: readCatalog()}},
  initAsync: false,
  keySeparator: false,
  nsSeparator: false,
  returnEmptyString: false,
  // Callers set textContent or build markup themselves: escaping is theirs.
  interpolation: {prefix: '{', suffix: '}', escapeValue: false},
});

export function t(text, ...args) {
  return i18next.t(text, {...args});
}
