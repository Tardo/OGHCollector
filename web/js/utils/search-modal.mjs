// Copyright Alexandre D. Díaz

// Wires the type-anywhere-to-search overlay on module/committer pages:
// any printable keydown opens it and focuses the input, Escape closes it.
export function bindSearchModal(modalId, componentTag) {
  const modal = document.getElementById(modalId);
  const input = () => modal.querySelector(componentTag).query('input');
  const tip = document.getElementById('search-tip');
  if (tip && localStorage.getItem('ommd_search_tip_dismissed') !== '1') {
    tip.classList.remove('d-none');
    tip.querySelector('button').addEventListener('click', () => {
      localStorage.setItem('ommd_search_tip_dismissed', '1');
      tip.remove();
    });
  }

  document.body.addEventListener('keydown', ev => {
    if (ev.ctrlKey || ev.altKey || ev.metaKey) {
      return;
    }
    if (modal.classList.contains('d-none')) {
      if (ev.key.length === 1) {
        modal.classList.remove('d-none');
        input().focus();
      }
    } else if (ev.code === 'Escape') {
      modal.classList.add('d-none');
      input().value = '';
    }
  });
}
