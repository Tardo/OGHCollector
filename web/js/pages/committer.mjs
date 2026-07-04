// Copyright Alexandre D. Díaz
import '@app/components/committer-search';
import '@scss/pages/committer.scss';

document.body.addEventListener('keydown', ev => {
  if (ev.ctrlKey || ev.altKey || ev.metaKey) {
    return;
  }
  const search_modal = document.getElementById('committer_search');
  if (search_modal.classList.contains('d-none')) {
    if (ev.key.length !== 1) {
      return;
    }
    search_modal.classList.remove('d-none');
    const search_comp = search_modal.querySelector('mirlo-committer-search');
    const input = search_comp.query('input');
    input.focus();
  } else if (ev.code === 'Escape') {
    search_modal.classList.add('d-none');
    const search_comp = search_modal.querySelector('mirlo-committer-search');
    const input = search_comp.query('input');
    input.value = '';
  }
});
