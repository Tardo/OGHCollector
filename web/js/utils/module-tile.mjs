// Copyright 2026 Alexandre D. Díaz

// Shared "module tile" link (icon + name + subtitle), the building block of
// the .module-tile-grid used on /favorites (favorites + pack contents) and
// /pack (shared pack view).
export function createModuleTileLink(mod, name, meta) {
  const link = document.createElement('a');
  link.className = 'module-tile-link';
  link.href = `/module/${encodeURIComponent(mod.org)}/${encodeURIComponent(mod.technical_name)}`;

  const icon = document.createElement('img');
  icon.className = 'module-tile-icon';
  icon.loading = 'lazy';
  icon.alt = '';
  // Modules without an icon file 404 - swap in the same generic package
  // glyph used as a fallback on the module page itself, instead of just
  // leaving a blank gap where the icon should be.
  icon.onerror = () => {
    const fallback = document.createElement('span');
    fallback.className = 'module-tile-icon module-tile-icon-generic';
    fallback.setAttribute('aria-hidden', 'true');
    fallback.textContent = '\u{1F4E6}';
    icon.replaceWith(fallback);
  };
  icon.src = `/common/odoo/module/${encodeURIComponent(mod.org)}/${encodeURIComponent(mod.technical_name)}/icon`;
  link.appendChild(icon);

  const body = document.createElement('span');
  body.className = 'module-tile-body';
  const name_el = document.createElement('span');
  name_el.className = 'module-tile-name';
  name_el.textContent = name;
  body.appendChild(name_el);
  const meta_el = document.createElement('span');
  meta_el.className = 'module-tile-meta';
  meta_el.textContent = meta;
  body.appendChild(meta_el);
  link.appendChild(body);

  return link;
}
