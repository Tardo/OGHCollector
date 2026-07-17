// Copyright Alexandre D. Díaz

// Wires the type-anywhere-to-search overlay shared by the module/committer/
// dashboard quick-search (see e.g. pages/module.html's #module_search):
// any printable keydown opens it and focuses the input, Escape closes it.
export function bindSearchModal(modalId, componentTag) {
  const modal = document.getElementById(modalId);
  const input = () => modal.querySelector(componentTag).query('input');

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
