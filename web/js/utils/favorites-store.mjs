// Copyright 2026 Alexandre D. Díaz

// All data lives in this browser's localStorage only - never sent to the
// server. See the warning banner on pages/favorites.html.
const FAVORITES_KEY = 'ommd_favorites';
const PACKS_KEY = 'ommd_packs';

function readList(key) {
  try {
    const parsed = JSON.parse(localStorage.getItem(key));
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function writeList(key, value) {
  localStorage.setItem(key, JSON.stringify(value));
}

function moduleKey(mod) {
  return `${mod.org}/${mod.technical_name}`;
}

// crypto.randomUUID() needs a secure context (https/localhost) - this app
// may be served over plain http on a LAN, so use a native fallback instead.
function makeId() {
  return `${Date.now().toString(36)}${Math.random().toString(36).slice(2)}`;
}

export function getFavorites() {
  return readList(FAVORITES_KEY);
}

export function isFavorite(mod) {
  return getFavorites().some(m => moduleKey(m) === moduleKey(mod));
}

// Returns the new favorited state (true = just added, false = just removed).
export function toggleFavorite(mod) {
  const favorites = getFavorites();
  const idx = favorites.findIndex(m => moduleKey(m) === moduleKey(mod));
  if (idx === -1) {
    favorites.push(mod);
  } else {
    favorites.splice(idx, 1);
  }
  writeList(FAVORITES_KEY, favorites);
  return idx === -1;
}

export function removeFavorite(mod) {
  writeList(
    FAVORITES_KEY,
    getFavorites().filter(m => moduleKey(m) !== moduleKey(mod)),
  );
}

export function getPacks() {
  return readList(PACKS_KEY);
}

// Packs are built for one Odoo version (module availability/size/export all
// depend on it) - odoo_version is required at creation, not settable later
// except by converting a shared pack on /pack (see pages/pack.mjs).
export function createPack(name, odoo_version) {
  const packs = getPacks();
  const pack = {id: makeId(), name, odoo_version, modules: []};
  packs.push(pack);
  writeList(PACKS_KEY, packs);
  return pack;
}

export function renamePack(id, name) {
  const packs = getPacks();
  const pack = packs.find(p => p.id === id);
  if (pack) {
    pack.name = name;
    writeList(PACKS_KEY, packs);
  }
}

export function deletePack(id) {
  writeList(
    PACKS_KEY,
    getPacks().filter(p => p.id !== id),
  );
}

export function isInPack(id, mod) {
  const pack = getPacks().find(p => p.id === id);
  return pack ? pack.modules.some(m => moduleKey(m) === moduleKey(mod)) : false;
}

export function addToPack(id, mod) {
  const packs = getPacks();
  const pack = packs.find(p => p.id === id);
  if (pack && !pack.modules.some(m => moduleKey(m) === moduleKey(mod))) {
    pack.modules.push(mod);
    writeList(PACKS_KEY, packs);
  }
}

export function removeFromPack(id, mod) {
  const packs = getPacks();
  const pack = packs.find(p => p.id === id);
  if (pack) {
    pack.modules = pack.modules.filter(m => moduleKey(m) !== moduleKey(mod));
    writeList(PACKS_KEY, packs);
  }
}

// Packs are shared as a self-contained URL (no server-side pack storage
// exists - see the module comment at the top of this file), so the ?d=
// param must carry the whole pack. Keep it minimal: name + [org,
// technical_name] pairs only, no display name (the pack page re-resolves
// that from the DB via /pack/info anyway).
export function encodePackForShare(pack) {
  const payload = JSON.stringify({
    n: pack.name,
    v: pack.odoo_version,
    m: pack.modules.map(mod => [mod.org, mod.technical_name]),
  });
  return btoa(unescape(encodeURIComponent(payload)))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '');
}

// Returns null for anything malformed rather than throwing - this decodes
// user-supplied URL input.
export function decodeSharedPack(encoded) {
  try {
    const base64 = encoded.replace(/-/g, '+').replace(/_/g, '/');
    const payload = JSON.parse(decodeURIComponent(escape(atob(base64))));
    if (typeof payload.n !== 'string' || !Array.isArray(payload.m)) {
      return null;
    }
    return {
      name: payload.n,
      // Absent on links shared before packs carried a version - callers
      // treat a missing odoo_version as "resolve against latest", same as
      // pre-version behavior.
      odoo_version: typeof payload.v === 'string' ? payload.v : undefined,
      modules: payload.m
        .filter(
          p =>
            Array.isArray(p) &&
            p.length === 2 &&
            p.every(x => typeof x === 'string'),
        )
        .map(([org, technical_name]) => ({
          org,
          technical_name,
        })),
    };
  } catch {
    return null;
  }
}

// ponytail: no test framework exists anywhere in web/js - this round-trip
// is the one place a URL-safe-base64 off-by-one would hide, so check it
// once at import time instead of adding a runner for a single assertion.
{
  const sample = {
    name: 'Sample Pack',
    odoo_version: '17.0',
    modules: [{org: 'OCA', technical_name: 'sale_x'}],
  };
  const decoded = decodeSharedPack(encodePackForShare(sample));
  const ok =
    decoded?.name === sample.name &&
    decoded.odoo_version === sample.odoo_version &&
    decoded.modules.length === 1 &&
    decoded.modules[0].org === 'OCA' &&
    decoded.modules[0].technical_name === 'sale_x';
  if (!ok) {
    console.error('encodePackForShare/decodeSharedPack round-trip broken');
  }
}
