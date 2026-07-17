// Copyright 2026 Alexandre D. Díaz

// Shared "processing..."/error indicator for the doodba tools' drag-and-drop
// panels: swaps the instructional text and dims the panel while a request
// is in flight, so selecting a file doesn't look like a no-op.
export function setDragPanelProcessing(panel_el, active) {
  const text_el = panel_el.querySelector('.no_mouse');
  if (active) {
    text_el.dataset.origText ??= text_el.textContent;
    text_el.textContent = 'Processing…';
    panel_el.style.pointerEvents = 'none';
    panel_el.style.opacity = '0.6';
    panel_el.style.borderColor = '';
  } else {
    if (text_el.dataset.origText) {
      text_el.textContent = text_el.dataset.origText;
    }
    panel_el.style.pointerEvents = '';
    panel_el.style.opacity = '';
  }
}

// Leaves the panel usable (undimmed) so the user can retry, with the error
// replacing the instructional text until the next attempt resets it.
export function showDragPanelError(panel_el, message) {
  const text_el = panel_el.querySelector('.no_mouse');
  text_el.dataset.origText ??= text_el.textContent;
  panel_el.style.pointerEvents = '';
  panel_el.style.opacity = '';
  panel_el.style.borderColor = '#e05252';
  text_el.textContent = `⚠️ ${message} Click or drag to try again.`;
}

// file.type (MIME) is unreliable for YAML - browsers report
// application/x-yaml, text/yaml, or '' depending on OS/browser, so a
// suffix check on `type` silently rejects valid files. Match by extension
// instead, same as the file-picker's `accept=".yaml,.yml"`.
export function isYamlFile(file) {
  return Boolean(file) && /\.ya?ml$/i.test(file.name);
}
