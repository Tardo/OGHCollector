// Copyright 2026 Alexandre D. Díaz
import '@app/components/module-search';
import '@scss/pages/modules.scss';

document.querySelectorAll('.pr-filter-group').forEach(group => {
  const list = group.closest('.tab-pane')?.querySelector('.pr-list');
  if (!list) {
    return;
  }
  group.addEventListener('click', ev => {
    const badge = ev.target.closest('.pr-filter');
    if (!badge) {
      return;
    }
    const was_active = badge.classList.contains('active');
    group
      .querySelectorAll('.pr-filter')
      .forEach(b => b.classList.remove('active'));
    list.classList.remove('filter-fresh', 'filter-rotting', 'filter-rotten');
    if (!was_active) {
      badge.classList.add('active');
      list.classList.add(`filter-${badge.dataset.filter}`);
    }
  });
});
