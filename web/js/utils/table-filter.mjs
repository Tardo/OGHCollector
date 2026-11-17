// Copyright 2026 Alexandre D. Díaz
// Text filter shared by the server-rendered lists (osv, migration, projects):
// a row matches when its `data-search` contains the lowercased input text.

export function filterTerm(input) {
  return input ? input.value.trim().toLowerCase() : '';
}

// Sets `hidden` on every row by `match(row)`; returns whether any stays visible.
export function filterRows(rows, match) {
  let any_visible = false;
  for (const row of rows) {
    row.hidden = !match(row);
    any_visible ||= !row.hidden;
  }
  return any_visible;
}

export function matchesTerm(row, term) {
  return term === '' || row.dataset.search.includes(term);
}
