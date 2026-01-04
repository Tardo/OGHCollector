// Copyright Alexandre D. Díaz
import '@app/components/module-search';
import '@scss/pages/module.scss';

document.body.addEventListener('keydown', ev => {
  if (ev.ctrlKey || ev.altKey) {
    return;
  }
  const search_modal = document.getElementById('module_search');
  if (search_modal.classList.contains('d-none')) {
    search_modal.classList.remove('d-none');
    const search_comp = search_modal.querySelector('mirlo-module-search');
    const input = search_comp.query('input');
    input.focus();
  } else if (ev.code === 'Escape') {
    search_modal.classList.add('d-none');
    const search_comp = search_modal.querySelector('mirlo-module-search');
    const input = search_comp.query('input');
    input.value = '';
  }
});
