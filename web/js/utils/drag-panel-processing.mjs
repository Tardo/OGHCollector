// Copyright 2026 Alexandre D. Díaz

// Shared "processing..."/error indicator for the doodba tools' drag-and-drop
// panels: toggles CSS classes (styled in _doodba-drag-panel.scss) that swap
// the instructional text and dim the panel while a request is in flight, so
// selecting a file doesn't look like a no-op.
export function setDragPanelProcessing(panel_el, active) {
  const text_el = panel_el.querySelector('.no_mouse');
  // A drop always leads here or to showDragPanelError - clearing drag-over
  // in both keeps that transient state from getting stuck on either path.
  panel_el.classList.remove('drag-over');
  panel_el.classList.toggle('processing', active);
  if (active) {
    text_el.dataset.origText ??= text_el.textContent;
    text_el.textContent = 'Processing…';
    panel_el.classList.remove('has-error');
  } else if (text_el.dataset.origText) {
    text_el.textContent = text_el.dataset.origText;
  }
}

// Leaves the panel usable (undimmed) so the user can retry, with the error
// replacing the instructional text until the next attempt resets it.
export function showDragPanelError(panel_el, message) {
  const text_el = panel_el.querySelector('.no_mouse');
  text_el.dataset.origText ??= text_el.textContent;
  panel_el.classList.remove('drag-over', 'processing');
  panel_el.classList.add('has-error');
  const icon = document.createElement('span');
  icon.className = 'material-icons';
  icon.setAttribute('aria-hidden', 'true');
  icon.textContent = 'warning';
  text_el.replaceChildren(icon, ` ${message} Click or drag to try again.`);
}

// file.type (MIME) is unreliable for YAML - browsers report
// application/x-yaml, text/yaml, or '' depending on OS/browser, so a
// suffix check on `type` silently rejects valid files. Match by extension
// instead, same as the file-picker's `accept=".yaml,.yml"`.
export function isYamlFile(file) {
  return Boolean(file) && /\.ya?ml$/i.test(file.name);
}
