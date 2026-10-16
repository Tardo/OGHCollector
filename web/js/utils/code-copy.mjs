// Copyright 2025 Alexandre D. Díaz
// "Copy" button ([data-copy]) of a .code-panel: copies the panel's <pre>.
// Outside a secure context (no clipboard API) it selects the snippet for
// Ctrl+C instead, like the workflow editor.
document.addEventListener('click', async ev => {
  const btn = ev.target.closest('[data-copy]');
  if (!btn) {
    return;
  }
  const code = btn.closest('.code-panel').querySelector('pre');
  try {
    await navigator.clipboard.writeText(code.textContent);
    btn.textContent = 'Copied!';
  } catch {
    window.getSelection().selectAllChildren(code);
    btn.textContent = 'Press Ctrl+C';
  }
  setTimeout(() => {
    btn.textContent = 'Copy';
  }, 1500);
});
