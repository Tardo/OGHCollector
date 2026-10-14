// Copyright 2026 Alexandre D. Díaz
//
// Workflow editor: a node graph (Drawflow) over the collector's workflow
// model. Node types, fields and triggers come from the server
// (`/workflows/data`, the collector's own tables), every change is
// re-checked by the collector's parser and rules (`/workflows/parse`), and
// the result is YAML to copy by hand: nothing is saved.
import '@scss/pages/workflows.scss';
import Drawflow from 'drawflow';
import * as yaml from 'js-yaml';

const $ = id => document.getElementById(id);
const els = {
  list: $('wf-list'),
  file_error: $('wf-file-error'),
  name: $('wf-name'),
  trigger: $('wf-trigger'),
  palette: $('wf-palette'),
  add: $('wf-add'),
  start: $('wf-start'),
  remove: $('wf-delete'),
  relayout: $('wf-layout-btn'),
  canvas: $('wf-canvas'),
  props: $('wf-props'),
  status: $('wf-status'),
  errors: $('wf-errors'),
  yaml: $('wf-yaml'),
  copy: $('wf-copy'),
  download: $('wf-download'),
  vars: $('wf-vars'),
  new_btn: $('wf-new'),
  import_open: $('wf-import-open'),
  import_dialog: $('wf-import'),
  import_text: $('wf-import-text'),
  import_error: $('wf-import-error'),
  import_load: $('wf-import-load'),
};

let meta = null;
let types = {};
let library = [];
let editor = null;
// Drawflow ids (as strings) of the start node and the selected one.
let start_id = null;
let selected_id = null;
let current_yaml = '';
// What Copy gives: the workflow alone, indented to go under an existing
// `workflows:` (a second `workflows:` key would replace or break the file's).
let current_snippet = '';
let valid = false;
let validate_timer = null;
let validate_seq = 0;
// While a workflow is being loaded, connection events aren't edits.
let loading = false;

const outputsOf = type => types[type]?.outputs ?? ['next'];
const isEdge = field => meta.fields[field]?.kind === 'edge';
const flowData = () => editor.export().drawflow[editor.module].data;

function esc(text) {
  const span = document.createElement('span');
  span.textContent = String(text ?? '');
  return span.innerHTML;
}

function el(tag, attrs = {}, text = null) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    node.setAttribute(key, value);
  }
  if (text !== null) {
    node.textContent = text;
  }
  return node;
}

// Node card: everything from the workflow is escaped (it comes from users
// and from collector.yaml).
function nodeHtml(data) {
  return `<div class="wf-node-type">${esc(data.type)}</div><div class="wf-node-id">${esc(data.id)}</div>`;
}

function nodeClass(type) {
  return /^[a-z_]+$/.test(type) ? `wf-type-${type}` : 'wf-type-unknown';
}

function addNode(data, x, y) {
  const id = editor.addNode(
    data.type,
    1,
    outputsOf(data.type).length,
    x,
    y,
    nodeClass(data.type),
    data,
    nodeHtml(data),
  );
  return String(id);
}

// A staircase: each column (the longest path from the start, so merges land
// after both branches) a step right and down, nodes sharing a column stacked;
// unreachable nodes in a last column.
function layout(nodes) {
  const by_id = new Map(nodes.map(n => [n.id, n]));
  const depth = new Map();
  if (nodes.length) {
    depth.set(nodes[0].id, 0);
  }
  for (let round = 0; round < nodes.length; round++) {
    for (const node of nodes) {
      if (!depth.has(node.id)) {
        continue;
      }
      for (const out of outputsOf(node.type)) {
        const target = node[out];
        if (by_id.has(target)) {
          depth.set(
            target,
            Math.max(depth.get(target) ?? 0, depth.get(node.id) + 1),
          );
        }
      }
    }
  }
  const last = Math.max(0, ...depth.values()) + 1;
  const rows = new Map();
  const pos = new Map();
  for (const node of nodes) {
    const col = depth.has(node.id) ? depth.get(node.id) : last;
    const row = rows.get(col) ?? 0;
    rows.set(col, row + 1);
    pos.set(node.id, {x: 40 + col * 150, y: 40 + col * 80 + row * 110});
  }
  return pos;
}

// Drawflow scales its canvas around its center: a canvas point p shows at
// center + (p - center) * zoom + translate.
function setView(zoom, tx, ty) {
  Object.assign(editor, {
    zoom,
    zoom_last_value: zoom,
    canvas_x: tx,
    canvas_y: ty,
  });
  editor.precanvas.style.transform = `translate(${tx}px, ${ty}px) scale(${zoom})`;
}

// Zooms (down to 50%) and pans so every node is in view.
function fitView() {
  const nodes = Object.values(flowData());
  const pre = editor.precanvas;
  if (!nodes.length) {
    setView(1, 0, 0);
    return;
  }
  const box = id => $(`node-${id}`);
  const min_x = Math.min(...nodes.map(n => n.pos_x));
  const min_y = Math.min(...nodes.map(n => n.pos_y));
  const max_x = Math.max(
    ...nodes.map(n => n.pos_x + (box(n.id)?.offsetWidth ?? 170)),
  );
  const max_y = Math.max(
    ...nodes.map(n => n.pos_y + (box(n.id)?.offsetHeight ?? 60)),
  );
  const margin = 24;
  const fit = Math.min(
    (els.canvas.clientWidth - 2 * margin) / (max_x - min_x),
    (els.canvas.clientHeight - 2 * margin) / (max_y - min_y),
  );
  const zoom = Math.min(1, Math.max(0.5, fit));
  const cx = pre.clientWidth / 2;
  const cy = pre.clientHeight / 2;
  setView(
    zoom,
    margin - cx - (min_x - cx) * zoom,
    margin - cy - (min_y - cy) * zoom,
  );
}

function markStart() {
  els.canvas
    .querySelectorAll('.drawflow-node.wf-start')
    .forEach(node => node.classList.remove('wf-start'));
  if (start_id !== null) {
    $(`node-${start_id}`)?.classList.add('wf-start');
  }
}

function loadWorkflow(name, workflow) {
  loading = true;
  editor.clear();
  els.name.value = name;
  els.trigger.value = workflow.on;
  refreshTrigger();
  const ids = new Map();
  const pos = layout(workflow.nodes);
  for (const node of workflow.nodes) {
    const fields = {};
    for (const [key, value] of Object.entries(node)) {
      if (!['id', 'type'].includes(key) && !isEdge(key)) {
        fields[key] = value;
      }
    }
    const at = pos.get(node.id);
    ids.set(
      node.id,
      addNode({id: node.id, type: node.type, fields}, at.x, at.y),
    );
  }
  for (const node of workflow.nodes) {
    outputsOf(node.type).forEach((out, index) => {
      if (ids.has(node[out])) {
        editor.addConnection(
          ids.get(node.id),
          ids.get(node[out]),
          `output_${index + 1}`,
          'input_1',
        );
      }
    });
  }
  start_id = workflow.nodes.length ? ids.get(workflow.nodes[0].id) : null;
  selected_id = null;
  loading = false;
  markStart();
  fitView();
  renderProps();
  regenerate();
}

// The graph as a workflow: the start node first (the collector starts at the
// first one), then the rest depth-first along their outputs, then any node
// nothing reaches, in creation order.
function toWorkflow() {
  const data = flowData();
  const all = Object.values(data).sort((a, b) => a.id - b.id);
  const target = (node, index) => {
    const conn = node.outputs[`output_${index + 1}`]?.connections?.[0];
    return conn ? data[conn.node] : undefined;
  };
  const order = [];
  const seen = new Set();
  const visit = node => {
    if (!node || seen.has(String(node.id))) {
      return;
    }
    seen.add(String(node.id));
    order.push(node);
    outputsOf(node.data.type).forEach((_, index) => visit(target(node, index)));
  };
  visit(start_id !== null ? data[start_id] : undefined);
  all.forEach(visit);
  const nodes = order.map(node => {
    const out = {id: node.data.id, type: node.data.type};
    for (const field of types[node.data.type]?.fields ?? []) {
      const value = node.data.fields?.[field];
      if (
        isEdge(field) ||
        value === undefined ||
        value === null ||
        value === ''
      ) {
        continue;
      }
      out[field] = meta.fields[field].kind === 'number' ? Number(value) : value;
    }
    outputsOf(node.data.type).forEach((field, index) => {
      const next = target(node, index);
      if (next) {
        out[field] = next.data.id;
      }
    });
    return out;
  });
  return {on: els.trigger.value, nodes};
}

function workflowName() {
  return els.name.value.trim() || 'my_workflow';
}

function regenerate() {
  if (loading) {
    return;
  }
  const workflow = {[workflowName()]: toWorkflow()};
  const options = {lineWidth: -1, noRefs: true};
  current_yaml = yaml.dump({workflows: workflow}, options);
  current_snippet = yaml.dump(workflow, options).replace(/^(?=.)/gm, '  ');
  els.yaml.textContent = current_yaml;
  valid = false;
  setYamlButtons();
  els.status.textContent = 'Checking…';
  els.status.className = 'wf-status';
  clearTimeout(validate_timer);
  validate_timer = setTimeout(validate, 350);
}

function showErrors(errors) {
  els.errors.textContent = '';
  for (const error of errors) {
    els.errors.append(el('li', {}, error));
  }
  els.status.textContent = errors.length
    ? `${errors.length} problem${errors.length > 1 ? 's' : ''}`
    : 'Valid';
  els.status.className = `wf-status ${errors.length ? 'text-danger' : 'text-success'}`;
}

// The collector's parser and rules decide; Copy/Download only offer text
// that just passed them.
async function validate() {
  const seq = ++validate_seq;
  const text = current_yaml;
  try {
    const res = await fetch('/workflows/parse', {
      method: 'POST',
      headers: {'Content-Type': 'text/plain; charset=utf-8'},
      body: text,
    });
    const body = res.ok
      ? await res.json()
      : {errors: [`The server answered ${res.status}`]};
    if (seq !== validate_seq) {
      return;
    }
    valid = body.errors.length === 0 && text === current_yaml;
    showErrors(body.errors);
  } catch (err) {
    if (seq === validate_seq) {
      showErrors([`Can't check it: ${err}`]);
    }
  }
  setYamlButtons();
}

function setYamlButtons() {
  els.copy.disabled = !valid;
  els.download.disabled = !valid;
}

function setNodeButtons() {
  els.start.disabled = selected_id === null || selected_id === start_id;
  els.remove.disabled = selected_id === null;
}

// One link per output (`next`, or an `if`'s `then`/`else`): a new one
// replaces the old.
function keepOnly(output_id, output_class, input_id) {
  const node = editor.getNodeFromId(output_id);
  for (const conn of [...(node.outputs[output_class]?.connections ?? [])]) {
    if (String(conn.node) !== String(input_id)) {
      editor.removeSingleConnection(
        output_id,
        conn.node,
        output_class,
        conn.output,
      );
    }
  }
}

function setOutput(output_id, output_class, input_id) {
  keepOnly(output_id, output_class, input_id);
  if (input_id) {
    editor.addConnection(output_id, input_id, output_class, 'input_1');
  }
}

function updateNodeData(data) {
  editor.updateNodeDataFromId(selected_id, data);
  const label = document.querySelector(`#node-${selected_id} .wf-node-id`);
  if (label) {
    label.textContent = data.id;
  }
  regenerate();
}

function fieldControl(field, value, onChange) {
  const spec = meta.fields[field];
  let control;
  if (spec.kind === 'enum') {
    control = el('select', {class: 'form-select form-select-sm'});
    control.append(el('option', {value: ''}, '(default)'));
    for (const option of spec.values) {
      control.append(el('option', {value: option}, option));
    }
    control.value = value ?? '';
    control.addEventListener('change', () => onChange(control.value));
    return control;
  }
  if (spec.kind === 'template') {
    const rows = ['prompt', 'patch', 'body', 'system'].includes(field) ? 6 : 2;
    control = el('textarea', {
      class: 'form-control form-control-sm font-monospace',
      rows: String(rows),
      spellcheck: 'false',
    });
  } else {
    control = el('input', {
      class: 'form-control form-control-sm',
      type: spec.kind === 'number' ? 'number' : 'text',
      spellcheck: 'false',
    });
    if (spec.kind === 'number') {
      control.min = '1';
    }
    if (spec.kind === 'condition') {
      control.classList.add('font-monospace');
    }
  }
  control.value = value ?? '';
  control.addEventListener('input', () => onChange(control.value));
  return control;
}

function renderProps() {
  els.props.textContent = '';
  setNodeButtons();
  if (selected_id === null) {
    els.props.append(
      el(
        'p',
        {class: 'text-body-secondary small'},
        'Select a node to edit it.',
      ),
    );
    return;
  }
  const node = editor.getNodeFromId(selected_id);
  const data = structuredClone(node.data);
  data.fields ??= {};
  const type = types[data.type];
  const head = el('div', {class: 'wf-props-head'});
  head.append(el('span', {class: 'badge text-bg-secondary'}, data.type));
  if (selected_id === start_id) {
    head.append(el('span', {class: 'badge text-bg-primary'}, 'start'));
  }
  els.props.append(
    head,
    el(
      'p',
      {class: 'small text-body-secondary'},
      type?.description ?? 'Unknown node type',
    ),
  );

  const id_label = el('label', {class: 'wf-prop'});
  id_label.append(el('span', {class: 'wf-prop-name'}, 'id *'));
  const id_input = el('input', {
    class: 'form-control form-control-sm',
    type: 'text',
    spellcheck: 'false',
  });
  id_input.value = data.id;
  id_input.addEventListener('input', () => {
    data.id = id_input.value.trim();
    updateNodeData(data);
  });
  id_label.append(id_input);
  els.props.append(id_label);

  const required = type?.required ?? [];
  for (const field of type?.fields ?? []) {
    if (isEdge(field)) {
      continue;
    }
    const label = el('label', {class: 'wf-prop'});
    label.append(
      el(
        'span',
        {class: 'wf-prop-name'},
        required.includes(field) ? `${field} *` : field,
      ),
    );
    label.append(
      fieldControl(field, data.fields[field], value => {
        data.fields[field] = value;
        updateNodeData(data);
      }),
    );
    label.append(
      el('span', {class: 'wf-prop-help'}, meta.fields[field].description),
    );
    els.props.append(label);
  }

  // Links, also settable here (not only by dragging).
  const others = Object.values(flowData()).filter(
    other => String(other.id) !== selected_id,
  );
  outputsOf(data.type).forEach((field, index) => {
    const output_class = `output_${index + 1}`;
    const label = el('label', {class: 'wf-prop'});
    label.append(el('span', {class: 'wf-prop-name'}, `${field} →`));
    const select = el('select', {class: 'form-select form-select-sm'});
    select.append(el('option', {value: ''}, '(end)'));
    for (const other of others) {
      select.append(el('option', {value: String(other.id)}, other.data.id));
    }
    select.value = String(
      node.outputs[output_class]?.connections?.[0]?.node ?? '',
    );
    select.addEventListener('change', () =>
      setOutput(selected_id, output_class, select.value),
    );
    label.append(
      select,
      el('span', {class: 'wf-prop-help'}, meta.fields[field].description),
    );
    els.props.append(label);
  });
}

function refreshTrigger() {
  const trigger = els.trigger.value;
  els.palette.textContent = '';
  for (const type of meta.node_types.filter(t =>
    t.triggers.includes(trigger),
  )) {
    els.palette.append(
      el('option', {value: type.type, title: type.description}, type.type),
    );
  }
  els.vars.textContent = '';
  for (const name of meta.variables[trigger] ?? []) {
    els.vars.append(el('li', {}, name));
  }
}

function uniqueId(base) {
  const taken = new Set(Object.values(flowData()).map(node => node.data.id));
  let id = base;
  for (let n = 2; taken.has(id); n++) {
    id = `${base}_${n}`;
  }
  return id;
}

// Where a new node goes: after the selected (or last added) node, else in
// the middle of what's visible.
function newNodePosition() {
  const data = flowData();
  const ids = Object.keys(data).sort((a, b) => a - b);
  const after = data[selected_id] ?? data[ids[ids.length - 1]];
  if (after) {
    return {x: after.pos_x + 200, y: after.pos_y + 50};
  }
  const pre = editor.precanvas;
  const zoom = editor.zoom || 1;
  const cx = pre.clientWidth / 2;
  const cy = pre.clientHeight / 2;
  return {
    x: cx + (els.canvas.clientWidth / 2 - editor.canvas_x - cx) / zoom - 85,
    y: cy + (els.canvas.clientHeight / 2 - editor.canvas_y - cy) / zoom - 30,
  };
}

function renderList(active) {
  els.list.textContent = '';
  els.list.append(
    el('option', {value: ''}, active === null ? '(new)' : 'Open…'),
  );
  library.forEach((item, index) => {
    const warn = item.error ? ' ⚠' : '';
    const option = el(
      'option',
      {value: String(index)},
      `${item.name} · ${item.origin}${warn}`,
    );
    if (item.error) {
      option.title = item.error;
    }
    els.list.append(option);
  });
  const index = library.findIndex(item => item.name === active);
  els.list.value = index >= 0 ? String(index) : '';
}

async function importYaml() {
  els.import_error.hidden = true;
  const res = await fetch('/workflows/parse', {
    method: 'POST',
    headers: {'Content-Type': 'text/plain; charset=utf-8'},
    body: els.import_text.value,
  });
  const body = res.ok
    ? await res.json()
    : {workflows: {}, errors: [`The server answered ${res.status}`]};
  const names = Object.keys(body.workflows);
  if (!names.length) {
    els.import_error.textContent =
      body.errors.join('\n') || 'No `workflows:` in it.';
    els.import_error.hidden = false;
    return;
  }
  for (const name of names) {
    const error =
      body.errors.find(e => e.startsWith(`workflow '${name}'`)) ?? null;
    library = library.filter(item => item.name !== name);
    library.unshift({
      name,
      origin: 'imported',
      workflow: body.workflows[name],
      error,
    });
  }
  els.import_dialog.close();
  loadWorkflow(names[0], body.workflows[names[0]]);
  renderList(names[0]);
}

function bindEvents() {
  editor.on('nodeSelected', id => {
    selected_id = String(id);
    renderProps();
  });
  editor.on('nodeUnselected', () => {
    selected_id = null;
    renderProps();
  });
  editor.on('connectionCreated', ({output_id, input_id, output_class}) => {
    if (String(output_id) === String(input_id)) {
      editor.removeSingleConnection(
        output_id,
        input_id,
        output_class,
        'input_1',
      );
      return;
    }
    keepOnly(output_id, output_class, input_id);
    regenerate();
    renderProps();
  });
  editor.on('connectionRemoved', () => {
    regenerate();
    renderProps();
  });
  editor.on('nodeRemoved', id => {
    const ids = Object.keys(flowData());
    if (start_id === String(id)) {
      start_id = ids.length ? ids.sort((a, b) => a - b)[0] : null;
    }
    if (selected_id === String(id)) {
      selected_id = null;
    }
    markStart();
    renderProps();
    regenerate();
  });

  els.list.addEventListener('change', () => {
    const item = library[Number(els.list.value)];
    if (item) {
      loadWorkflow(item.name, item.workflow);
      renderList(item.name);
    }
  });
  els.name.addEventListener('input', regenerate);
  els.trigger.addEventListener('change', () => {
    refreshTrigger();
    regenerate();
  });
  els.add.addEventListener('click', () => {
    const type = els.palette.value;
    const at = newNodePosition();
    const id = addNode({id: uniqueId(type), type, fields: {}}, at.x, at.y);
    if (start_id === null) {
      start_id = id;
      markStart();
    }
    regenerate();
  });
  els.start.addEventListener('click', () => {
    start_id = selected_id;
    markStart();
    renderProps();
    regenerate();
  });
  els.remove.addEventListener('click', () => {
    if (selected_id !== null) {
      editor.removeNodeId(`node-${selected_id}`);
    }
  });
  els.relayout.addEventListener('click', () =>
    loadWorkflow(workflowName(), toWorkflow()),
  );
  els.new_btn.addEventListener('click', () => {
    loadWorkflow('my_workflow', {on: meta.triggers[0], nodes: []});
    renderList(null);
  });
  els.copy.addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(current_snippet);
      els.copy.lastChild.textContent = ' Copied!';
    } catch {
      window.getSelection().selectAllChildren(els.yaml);
      els.copy.lastChild.textContent = ' Press Ctrl+C';
    }
    setTimeout(() => {
      els.copy.lastChild.textContent = ' Copy';
    }, 1500);
  });
  els.download.addEventListener('click', () => {
    const link = el('a', {
      href: URL.createObjectURL(new Blob([current_yaml], {type: 'text/yaml'})),
      download: `${workflowName()}.yaml`,
    });
    link.click();
    URL.revokeObjectURL(link.href);
  });
  els.import_open.addEventListener('click', () => {
    els.import_error.hidden = true;
    els.import_dialog.showModal();
  });
  els.import_load.addEventListener('click', importYaml);
}

async function init() {
  editor = new Drawflow(els.canvas);
  editor.reroute = false;
  editor.start();
  const res = await fetch('/workflows/data');
  const data = await res.json();
  meta = data.meta;
  types = Object.fromEntries(meta.node_types.map(type => [type.type, type]));
  for (const trigger of meta.triggers) {
    els.trigger.append(el('option', {value: trigger}, trigger));
  }
  if (data.file_error) {
    els.file_error.textContent = data.file_error;
    els.file_error.hidden = false;
  }
  library = data.workflows;
  bindEvents();
  const first = library.find(item => item.name === 'default') ?? library[0];
  if (first) {
    loadWorkflow(first.name, first.workflow);
  }
  renderList(first?.name ?? null);
}

init();
