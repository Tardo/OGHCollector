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
  enabled: $('wf-enabled'),
  palette: $('wf-palette'),
  add: $('wf-add'),
  start: $('wf-start'),
  remove: $('wf-delete'),
  relayout: $('wf-layout-btn'),
  canvas: $('wf-canvas'),
  balloon: $('wf-balloon'),
  status: $('wf-status'),
  errors: $('wf-errors'),
  yaml_toggle: $('wf-yaml-toggle'),
  yaml_body: $('wf-yaml-body'),
  yaml: $('wf-yaml'),
  copy: $('wf-copy'),
  download: $('wf-download'),
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
// The last check's problems by node name, shown on each card.
let node_problems = new Map();

const outputsOf = type => types[type]?.outputs ?? ['next'];
const isEdge = field => meta.fields[field]?.kind === 'edge';
const flowData = () => editor.export().drawflow[editor.module].data;
const contentOf = id =>
  $(`node-${id}`)?.querySelector('.drawflow_content_node');

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

// A card shows at most this many settings, then "+N more".
const CARD_ROWS = 3;

// The settings a card shows: the set ones, and the required ones still unset.
function cardRows(type, fields) {
  const rows = [];
  for (const field of types[type]?.fields ?? []) {
    const value = fields?.[field];
    if (isEdge(field)) {
      continue;
    }
    if (value === undefined || value === null || value === '') {
      if (types[type].required.includes(field)) {
        rows.push({field, value: null});
      }
      continue;
    }
    rows.push({field, value: String(value)});
  }
  return rows;
}

// A closed card's settings (empty when it has none).
function nodeSummary(data) {
  const conf = el('div', {class: 'wf-node-conf'});
  const rows = cardRows(data.type, data.fields);
  for (const {field, value} of rows.slice(0, CARD_ROWS)) {
    const row = el('div', {class: 'wf-node-row'});
    row.append(el('span', {class: 'wf-node-key'}, `${field}:`), ' ');
    if (value === null) {
      row.classList.add('wf-node-missing');
      row.append('required');
    } else {
      const lines = value.split('\n');
      row.title = value;
      row.append(lines[0] + (lines.length > 1 ? ' …' : ''));
    }
    conf.append(row);
  }
  if (rows.length > CARD_ROWS) {
    conf.append(
      el('div', {class: 'wf-node-more'}, `+${rows.length - CARD_ROWS} more`),
    );
  }
  return conf;
}

function nodeClass(type) {
  return /^[a-z_]+$/.test(type) ? `wf-type-${type}` : 'wf-type-unknown';
}

// Drawflow ids of the folded cards; the others show their form.
const folded = new Set();

function iconButton(icon, label) {
  const button = el('button', {
    type: 'button',
    class: 'wf-node-btn',
    'aria-label': label,
    title: label,
  });
  button.append(
    el('span', {class: 'material-icons', 'aria-hidden': 'true'}, icon),
  );
  return button;
}

// A card: its header (fold, type, info, name) and its form or, folded, a
// summary. Built as DOM: the text comes from users and collector.yaml.
function renderNode(id) {
  const content = contentOf(id);
  const data = structuredClone(editor.getNodeFromId(id).data);
  data.fields ??= {};
  const open = !folded.has(id);
  const fold = iconButton(
    open ? 'expand_less' : 'expand_more',
    open ? 'Fold' : 'Unfold',
  );
  fold.setAttribute('aria-expanded', String(open));
  fold.addEventListener('click', () => {
    if (open) {
      folded.add(id);
    } else {
      folded.delete(id);
    }
    renderNode(id);
    contentOf(id).querySelector('.wf-node-btn').focus();
  });
  const info = iconButton('info', 'About this node');
  hoverHelp(info, id, null, 0);
  for (const event of ['click', 'focus']) {
    info.addEventListener(event, () => showBalloon(id, null, info));
  }
  info.addEventListener('blur', hideBalloon);
  const head = el('div', {class: 'wf-node-head'});
  head.append(fold, el('span', {class: 'wf-node-type'}, data.type), info);
  const top = el('div');
  top.append(head, el('div', {class: 'wf-node-id'}, data.id));
  hoverHelp(top, id, null, HOVER_DELAY);
  content.textContent = '';
  content.append(
    top,
    el('ul', {class: 'wf-node-problems'}),
    open ? nodeForm(id, data) : nodeSummary(data),
  );
  $(`node-${id}`).classList.toggle('wf-open', open);
  markProblems(id);
}

// A card with problems: red, each one spelled out under its header.
function markProblems(id) {
  const name = editor.getNodeFromId(id).data.id;
  const problems = node_problems.get(name) ?? [];
  $(`node-${id}`).classList.toggle('wf-invalid', problems.length > 0);
  contentOf(id)
    .querySelector('.wf-node-problems')
    .replaceChildren(...problems.map(problem => el('li', {}, problem)));
}

function addNode(data, x, y) {
  const id = String(
    editor.addNode(
      data.type,
      1,
      outputsOf(data.type).length,
      x,
      y,
      nodeClass(data.type),
      data,
      '',
    ),
  );
  const content = contentOf(id);
  // Typing in the form isn't Drawflow's: its Delete would remove the node
  // (it only spares an INPUT/TEXTAREA last clicked, not a select or a field
  // reached by Tab) and its context menu would block right-click paste.
  content.addEventListener('keydown', e => e.stopPropagation());
  content.addEventListener('contextmenu', e => {
    if (e.target.closest('input, textarea, select')) {
      e.stopPropagation();
    }
  });
  resizes.observe($(`node-${id}`));
  renderNode(id);
  return id;
}

function moveNode(id, x, y) {
  Object.assign(editor.drawflow.drawflow[editor.module].data[id], {
    pos_x: x,
    pos_y: y,
  });
  Object.assign($(`node-${id}`).style, {left: `${x}px`, top: `${y}px`});
  editor.updateConnectionNodes(`node-${id}`);
}

// A grid in flow order (by the longest path from the start, so merges land
// after both branches; unreachable nodes last), row by row, with the cards'
// real sizes and as many columns as show them biggest on the canvas.
function arrange() {
  const nodes = editor.drawflow.drawflow[editor.module].data;
  const ids = Object.keys(nodes);
  if (!ids.length) {
    return;
  }
  const depth = new Map();
  if (nodes[start_id]) {
    depth.set(start_id, 0);
  }
  for (let round = 0; round < ids.length; round++) {
    for (const id of ids.filter(key => depth.has(key))) {
      for (const output of Object.values(nodes[id].outputs)) {
        for (const {node} of output.connections) {
          depth.set(node, Math.max(depth.get(node) ?? 0, depth.get(id) + 1));
        }
      }
    }
  }
  const rank = id => depth.get(id) ?? Infinity;
  const order = ids.sort((a, b) => rank(a) - rank(b) || a - b);
  const box = id => $(`node-${id}`);
  const col_width = Math.max(...order.map(id => box(id).offsetWidth)) + 60;
  const rowsOf = cols =>
    Array.from({length: Math.ceil(order.length / cols)}, (_, row) =>
      order.slice(row * cols, (row + 1) * cols),
    );
  const rowHeight = row =>
    Math.max(...row.map(id => box(id).offsetHeight)) + 40;
  const zoomFor = cols =>
    Math.min(
      1,
      (els.canvas.clientWidth - 48) / (cols * col_width),
      (els.canvas.clientHeight - 48) /
        rowsOf(cols).reduce((sum, row) => sum + rowHeight(row), 0),
    );
  let cols = 1;
  for (let more = 2; more <= order.length; more++) {
    if (zoomFor(more) >= zoomFor(cols)) {
      cols = more;
    }
  }
  // Too much to show at once: as many columns as fit across at the smallest
  // zoom, the rest further down (panned vertically).
  if (zoomFor(cols) < MIN_ZOOM) {
    const across = (els.canvas.clientWidth - 48) / (col_width * MIN_ZOOM);
    cols = Math.min(order.length, Math.max(1, Math.floor(across)));
  }
  let y = 40;
  for (const row of rowsOf(cols)) {
    row.forEach((id, index) => moveNode(id, 40 + index * col_width, y));
    y += rowHeight(row);
  }
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

const MIN_ZOOM = 0.5;

// Zooms (down to MIN_ZOOM) and pans so every node is in view, centered;
// what doesn't fit even then starts at the top left.
function fitView() {
  pristine = true;
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
    ...nodes.map(n => n.pos_x + (box(n.id)?.offsetWidth ?? 200)),
  );
  const max_y = Math.max(
    ...nodes.map(n => n.pos_y + (box(n.id)?.offsetHeight ?? 60)),
  );
  const margin = 24;
  const fit = Math.min(
    (els.canvas.clientWidth - 2 * margin) / (max_x - min_x),
    (els.canvas.clientHeight - 2 * margin) / (max_y - min_y),
  );
  const zoom = Math.min(1, Math.max(MIN_ZOOM, fit));
  const left = Math.max(
    margin,
    (els.canvas.clientWidth - (max_x - min_x) * zoom) / 2,
  );
  const top = Math.max(
    margin,
    (els.canvas.clientHeight - (max_y - min_y) * zoom) / 2,
  );
  const cx = pre.clientWidth / 2;
  const cy = pre.clientHeight / 2;
  setView(
    zoom,
    left - cx - (min_x - cx) * zoom,
    top - cy - (min_y - cy) * zoom,
  );
}

// Fitted, and nobody has touched the canvas since: the view follows size
// changes (the first check's problems on the cards and under the canvas, the
// YAML opening, the window), and so does the layout while it's ours
// (`auto_layout`; a workflow's saved positions, or a card someone moved, are
// left alone). Touching the canvas stops all that, so nothing moves while
// you work; the wand starts it again, with our layout.
let pristine = false;
let auto_layout = false;

function relayout() {
  auto_layout = true;
  arrange();
  fitView();
  regenerate();
}

const isPos = pos =>
  Array.isArray(pos) && pos.length === 2 && pos.every(Number.isFinite);

// Cards a workflow saved no position for, in a row under the rest.
function placeBelow(ids) {
  const data = editor.drawflow.drawflow[editor.module].data;
  const others = Object.keys(data).filter(id => !ids.includes(id));
  const box = id => $(`node-${id}`);
  let x = Math.min(...others.map(id => data[id].pos_x));
  const y =
    Math.max(...others.map(id => data[id].pos_y + box(id).offsetHeight)) + 60;
  for (const id of ids) {
    moveNode(id, x, y);
    x += box(id).offsetWidth + 60;
  }
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
  folded.clear();
  hideBalloon();
  els.name.value = name;
  els.trigger.value = workflow.on;
  els.enabled.checked = workflow.enabled !== false;
  refreshTrigger();
  const ids = new Map();
  const unplaced = [];
  for (const node of workflow.nodes) {
    const fields = {};
    for (const [key, value] of Object.entries(node)) {
      if (!['id', 'type', 'pos'].includes(key) && !isEdge(key)) {
        fields[key] = value;
      }
    }
    const [x, y] = isPos(node.pos) ? node.pos : [0, 0];
    const id = addNode({id: node.id, type: node.type, fields}, x, y);
    ids.set(node.id, id);
    if (!isPos(node.pos)) {
      unplaced.push(id);
    }
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
  // Its own layout when it saved one, ours otherwise.
  auto_layout = unplaced.length === workflow.nodes.length;
  if (auto_layout) {
    arrange();
  } else if (unplaced.length) {
    placeBelow(unplaced);
  }
  fitView();
  setNodeButtons();
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
    // The layout travels with it (the collector ignores it).
    out.pos = [Math.round(node.pos_x), Math.round(node.pos_y)];
    return out;
  });
  // On is the default: only "off" is written.
  const enabled = els.enabled.checked ? {} : {enabled: false};
  return {on: els.trigger.value, ...enabled, nodes};
}

function workflowName() {
  return els.name.value.trim() || 'my_workflow';
}

function regenerate() {
  if (loading) {
    return;
  }
  const workflow = {[workflowName()]: toWorkflow()};
  // Flow style from the nodes' fields down: `pos: [40, 60]`, one line.
  const options = {lineWidth: -1, noRefs: true};
  const text = yaml.dump({workflows: workflow}, {...options, flowLevel: 5});
  if (text === current_yaml) {
    // Nothing changed (a relayout that moved nothing): no new check.
    return;
  }
  current_yaml = text;
  current_snippet = yaml
    .dump(workflow, {...options, flowLevel: 4})
    .replace(/^(?=.)/gm, '  ');
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
  // "workflow 'w': node 'n': what's wrong" (also "node id 'n'", "through
  // node 'n'"): the card gets what follows its name.
  node_problems = new Map();
  for (const error of errors) {
    const name = error.match(/node(?: id)? '([^']*)'/)?.[1];
    if (name !== undefined) {
      const problem = error
        .replace(/^workflow '[^']*': /, '')
        .replace(/^node '[^']*': /, '');
      node_problems.set(name, [...(node_problems.get(name) ?? []), problem]);
    }
  }
  Object.keys(flowData()).forEach(markProblems);
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

// Only the card's name follows: rebuilding the form would lose the focus. The
// summary is rebuilt when the card folds.
function updateNodeData(id, data) {
  editor.updateNodeDataFromId(id, data);
  $(`node-${id}`).querySelector('.wf-node-id').textContent = data.id;
  regenerate();
}

// A condition built from rules (variable, operator, value), all or any of
// them, written as the Jinja expression the collector evaluates. An
// expression the rules can't express stays editable as text.
const OPERATORS = {
  '==': 'is',
  '!=': 'is not',
  contains: 'contains',
  'not contains': "doesn't contain",
  'starts with': 'starts with',
  'ends with': 'ends with',
  '>': '>',
  '<': '<',
  'is true': 'is true',
  'is false': 'is false',
};
const takesValue = op => !['is true', 'is false'].includes(op);
// `true`/`false`/integers as such (pr.number == 3); anything else, "18.0"
// included, as text.
const literal = value =>
  /^(true|false|-?\d+)$/.test(value) ? value : JSON.stringify(value);

function ruleExpr({variable, op, value}) {
  const text = JSON.stringify(value);
  switch (op) {
    case 'contains':
      return `${text} in ${variable}`;
    case 'not contains':
      return `${text} not in ${variable}`;
    case 'starts with':
      return `${variable} is startingwith(${text})`;
    case 'ends with':
      return `${variable} is endingwith(${text})`;
    case 'is true':
      return variable;
    case 'is false':
      return `not ${variable}`;
    default:
      return `${variable} ${op} ${literal(value)}`;
  }
}

const VAR = '[A-Za-z_][\\w.]*';
const STR = '"(?:[^"\\\\]|\\\\.)*"';
const END = '(?= (?:and|or) |$)';
// What ruleExpr writes, read back (first match wins).
const CLAUSES = [
  [
    `(${STR}) (not in|in) (${VAR})`,
    m => [m[3], m[2] === 'in' ? 'contains' : 'not contains', JSON.parse(m[1])],
  ],
  [
    `(${VAR}) is (startingwith|endingwith)\\((${STR})\\)`,
    m => [
      m[1],
      m[2] === 'startingwith' ? 'starts with' : 'ends with',
      JSON.parse(m[3]),
    ],
  ],
  [
    `(${VAR}) (==|!=|>|<) (${STR}|true|false|-?\\d+)${END}`,
    m => [m[1], m[2], m[3].startsWith('"') ? JSON.parse(m[3]) : m[3]],
  ],
  [`not (${VAR})${END}`, m => [m[1], 'is false', '']],
  [`(${VAR})${END}`, m => [m[1], 'is true', '']],
].map(([re, make]) => [new RegExp(`^${re}`), make]);

// {match: 'and'|'or', rules} or, for anything else, null.
function parseCondition(text) {
  let rest = text.trim();
  const rules = [];
  let match = null;
  try {
    while (rest) {
      const [m, make] = CLAUSES.map(([re, f]) => [rest.match(re), f]).find(
        ([hit]) => hit,
      ) ?? [null];
      if (!m) {
        return null;
      }
      const [variable, op, value] = make(m);
      rules.push({variable, op, value});
      rest = rest.slice(m[0].length);
      const joiner = rest.match(/^ (and|or) (?=.)/);
      if (rest && (!joiner || (match && joiner[1] !== match))) {
        return null;
      }
      match = joiner?.[1] ?? match;
      rest = rest.slice(joiner?.[0].length ?? 0);
    }
  } catch {
    // A string with an escape JSON doesn't know: leave it as text.
    return null;
  }
  return {match: match ?? 'and', rules};
}

// Variables a rule can test: plain values, not lists of objects or calls.
const ruleVariables = () =>
  (meta.variables[els.trigger.value] ?? []).filter(name => !/[(<[]/.test(name));

function conditionControl(value, onChange) {
  const box = el('div', {class: 'wf-cond'});
  const rules_box = el('div', {class: 'wf-cond-rules'});
  const raw = el('input', {
    class: 'form-control form-control-sm font-monospace',
    type: 'text',
    spellcheck: 'false',
    'aria-label': 'Condition (Jinja)',
  });
  raw.value = value ?? '';
  let state = parseCondition(raw.value);
  const emit = () => {
    raw.value = state.rules
      .filter(rule => !takesValue(rule.op) || rule.value !== '')
      .map(ruleExpr)
      .join(` ${state.match} `);
    onChange(raw.value);
  };
  const select = (options, current, label) => {
    const control = el('select', {
      class: 'form-select form-select-sm',
      'aria-label': label,
    });
    for (const [key, text] of options) {
      control.append(el('option', {value: key}, text));
    }
    control.value = current;
    return control;
  };
  const draw = () => {
    rules_box.textContent = '';
    if (!state) {
      rules_box.append(
        el(
          'p',
          {class: 'wf-cond-note'},
          'Written by hand: empty it to build it from rules.',
        ),
      );
      return;
    }
    if (state.rules.length > 1) {
      const match = select(
        [
          ['and', 'All of'],
          ['or', 'Any of'],
        ],
        state.match,
        'Rules to match',
      );
      match.addEventListener('change', () => {
        state.match = match.value;
        emit();
      });
      rules_box.append(match);
    }
    state.rules.forEach((rule, index) => {
      const row = el('div', {class: 'wf-cond-rule'});
      const names = ruleVariables();
      const variable = select(
        (names.includes(rule.variable) ? names : [rule.variable, ...names]).map(
          n => [n, n],
        ),
        rule.variable,
        'Variable',
      );
      const op = select(Object.entries(OPERATORS), rule.op, 'Operator');
      const input = el('input', {
        class: 'form-control form-control-sm',
        type: 'text',
        spellcheck: 'false',
        'aria-label': 'Value',
      });
      input.value = rule.value;
      input.hidden = !takesValue(rule.op);
      const remove = iconButton('close', 'Remove this rule');
      variable.addEventListener('change', () => {
        rule.variable = variable.value;
        emit();
      });
      op.addEventListener('change', () => {
        rule.op = op.value;
        input.hidden = !takesValue(rule.op);
        emit();
      });
      input.addEventListener('input', () => {
        rule.value = input.value;
        emit();
      });
      remove.addEventListener('click', () => {
        state.rules.splice(index, 1);
        draw();
        emit();
      });
      row.append(variable, op, remove, input);
      rules_box.append(row);
    });
    const add = el('button', {
      type: 'button',
      class: 'btn btn-sm btn-outline-secondary wf-cond-add',
    });
    add.append(
      el(
        'span',
        {class: 'material-icons', 'aria-hidden': 'true'},
        'filter_alt',
      ),
      ' Add rule',
    );
    add.addEventListener('click', () => {
      state.rules.push({
        variable: ruleVariables()[0],
        op: 'contains',
        value: '',
      });
      draw();
      rules_box.querySelector('.wf-cond-rule:last-of-type input').focus();
    });
    rules_box.append(add);
  };
  raw.addEventListener('input', () => {
    state = parseCondition(raw.value);
    draw();
    onChange(raw.value);
  });
  draw();
  box.append(rules_box, raw);
  return box;
}

function fieldControl(field, value, onChange) {
  const spec = meta.fields[field];
  let control;
  if (spec.kind === 'condition') {
    return conditionControl(value, onChange);
  }
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
    const rows = ['prompt', 'patch', 'body', 'system'].includes(field) ? 4 : 2;
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
  }
  control.value = value ?? '';
  control.addEventListener('input', () => onChange(control.value));
  return control;
}

const helpOf = field =>
  field === 'id'
    ? "This node's name: links and ai.<id> refer to it"
    : (meta.fields[field]?.description ?? '');

// One labelled control; its help shows in the balloon when the pointer rests
// on it, and is read out with the control.
function formField(id, name, control) {
  // A label around several controls would send its clicks to the first one.
  const label = el(control.tagName === 'DIV' ? 'div' : 'label', {
    class: 'wf-prop',
  });
  control.dataset.field = name;
  control.setAttribute('aria-description', helpOf(name));
  label.append(el('span', {class: 'wf-prop-name'}, name), control);
  hoverHelp(label, id, name, HOVER_DELAY);
  return label;
}

function nodeForm(id, data) {
  const form = el('div', {class: 'wf-node-form'});
  const type = types[data.type];
  const id_input = el('input', {
    class: 'form-control form-control-sm',
    type: 'text',
    spellcheck: 'false',
  });
  id_input.value = data.id;
  id_input.addEventListener('input', () => {
    data.id = id_input.value.trim();
    updateNodeData(id, data);
    refreshLinks();
  });
  form.append(formField(id, 'id', id_input));
  for (const field of type?.fields ?? []) {
    if (isEdge(field)) {
      continue;
    }
    const control = fieldControl(field, data.fields[field], value => {
      data.fields[field] = value;
      updateNodeData(id, data);
    });
    const label = formField(id, field, control);
    if (type.required.includes(field)) {
      label.firstChild.append(' *');
    }
    form.append(label);
  }

  // Links, also settable here (not only by dragging).
  const nodes = flowData();
  outputsOf(data.type).forEach((field, index) => {
    const select = el('select', {
      class: 'form-select form-select-sm',
      'data-output': `output_${index + 1}`,
    });
    fillLink(select, id, nodes);
    select.addEventListener('change', () =>
      setOutput(id, select.dataset.output, select.value),
    );
    const label = formField(id, field, select);
    label.firstChild.append(' →');
    form.append(label);
  });
  return form;
}

// A link select: the other nodes by their current names, and where that
// output points now. Refilled in place, so the focus stays put.
function fillLink(select, id, nodes) {
  select.replaceChildren(el('option', {value: ''}, '(end)'));
  for (const other of Object.values(nodes)) {
    if (String(other.id) !== id) {
      select.append(el('option', {value: String(other.id)}, other.data.id));
    }
  }
  select.value = String(
    nodes[id]?.outputs[select.dataset.output]?.connections?.[0]?.node ?? '',
  );
}

// Every card's link selects, after a node was added, renamed or removed, or
// a link drawn or removed.
function refreshLinks() {
  const nodes = flowData();
  els.canvas.querySelectorAll('select[data-output]').forEach(select => {
    fillLink(select, select.closest('.drawflow-node').id.slice(5), nodes);
  });
}

// The pointer rests this long on a card's header or a field before its help
// shows (the info button shows it at once).
const HOVER_DELAY = 500;
let hover_timer = null;
// What the balloon shows help for: a node and the element it points at.
let balloon_at = null;

function hoverHelp(target, id, field, delay) {
  target.addEventListener('mouseenter', () => {
    clearTimeout(hover_timer);
    hover_timer = setTimeout(() => showBalloon(id, field, target), delay);
  });
  target.addEventListener('mouseleave', hideBalloon);
}

function hideBalloon() {
  clearTimeout(hover_timer);
  balloon_at = null;
  els.balloon.hidden = true;
}

// Floating help: a node (what it does) or one of its fields (plus the
// variables a condition/template can use). Screen-space, beside the card, so
// it stays readable at any zoom.
function showBalloon(id, field, anchor) {
  const balloon = els.balloon;
  if (!anchor.isConnected) {
    hideBalloon();
    return;
  }
  const data = editor.getNodeFromId(id).data;
  balloon.textContent = '';
  if (field === null) {
    const head = el('div', {class: 'wf-balloon-head'});
    head.append(el('span', {class: 'badge text-bg-secondary'}, data.type));
    if (id === start_id) {
      head.append(el('span', {class: 'badge text-bg-primary'}, 'start'));
    }
    balloon.append(
      head,
      el('p', {}, types[data.type]?.description ?? 'Unknown node type'),
    );
  } else {
    const p = el('p');
    p.append(el('code', {}, field), ` ${helpOf(field)}`);
    balloon.append(p);
    if (['condition', 'template'].includes(meta.fields[field]?.kind)) {
      const vars = el('div', {class: 'wf-balloon-vars'});
      for (const name of meta.variables[els.trigger.value] ?? []) {
        vars.append(el('code', {}, name), ' ');
      }
      balloon.append(vars);
    }
  }
  balloon_at = {id, anchor};
  balloon.hidden = false;
  placeBalloon();
}

// Beside the card (right, else left), level with what it explains, clamped
// to the canvas; hidden while that is out of view.
function placeBalloon() {
  const balloon = els.balloon;
  if (!balloon_at || !balloon_at.anchor.isConnected) {
    return;
  }
  const box = balloon.parentElement.getBoundingClientRect();
  const view = els.canvas.getBoundingClientRect();
  const card = $(`node-${balloon_at.id}`).getBoundingClientRect();
  const anchor = balloon_at.anchor.getBoundingClientRect();
  const gap = 12;
  const right = card.right + gap + balloon.offsetWidth <= view.right;
  const left = right
    ? card.right + gap
    : Math.max(view.left + gap, card.left - gap - balloon.offsetWidth);
  const top = Math.max(
    view.top + gap,
    Math.min(anchor.top, view.bottom - gap - balloon.offsetHeight),
  );
  balloon.style.left = `${left - box.left}px`;
  balloon.style.top = `${top - box.top}px`;
  balloon.classList.toggle('wf-balloon-left', !right);
  balloon.style.visibility =
    anchor.bottom < view.top ||
    anchor.top > view.bottom ||
    card.right < view.left ||
    card.left > view.right
      ? 'hidden'
      : '';
}

// Cards change size (folding, a resized textarea): their dots move with
// them, and so does the balloon.
const resizes = new ResizeObserver(entries => {
  for (const {target} of entries) {
    if (target.isConnected && target.id.startsWith('node-')) {
      editor.updateConnectionNodes(target.id);
    }
  }
  if (pristine && auto_layout) {
    relayout();
  } else if (pristine) {
    fitView();
  }
  placeBalloon();
});

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
    const width = $(`node-${after.id}`)?.offsetWidth ?? 200;
    return {x: after.pos_x + width + 40, y: after.pos_y + 50};
  }
  const pre = editor.precanvas;
  const zoom = editor.zoom || 1;
  const cx = pre.clientWidth / 2;
  const cy = pre.clientHeight / 2;
  return {
    x: cx + (els.canvas.clientWidth / 2 - editor.canvas_x - cx) / zoom - 100,
    y: cy + (els.canvas.clientHeight / 2 - editor.canvas_y - cy) / zoom - 30,
  };
}

function renderList(active) {
  els.list.textContent = '';
  els.list.append(
    el('option', {value: ''}, active === null ? '(new)' : 'Open…'),
  );
  library.forEach((item, index) => {
    const warn =
      (item.workflow.enabled === false ? ' · off' : '') +
      (item.error ? ' ⚠' : '');
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
    setNodeButtons();
  });
  editor.on('nodeUnselected', () => {
    selected_id = null;
    setNodeButtons();
  });
  for (const event of ['mouseMove', 'zoom', 'translate', 'nodeMoved']) {
    editor.on(event, placeBalloon);
  }
  // A card put somewhere: the layout is someone's now, and the YAML says so.
  // Where it shows, that is: Drawflow 0.0.60 stores a dragged card's
  // position a step ahead of it (it reads offsetLeft after moving it).
  editor.on('nodeMoved', id => {
    const card = $(`node-${id}`);
    moveNode(
      String(id),
      Math.round(parseFloat(card.style.left)),
      Math.round(parseFloat(card.style.top)),
    );
    auto_layout = false;
    regenerate();
  });
  window.addEventListener('resize', placeBalloon);
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
    refreshLinks();
  });
  editor.on('connectionRemoved', () => {
    regenerate();
    refreshLinks();
  });
  editor.on('nodeRemoved', id => {
    const ids = Object.keys(flowData());
    if (start_id === String(id)) {
      start_id = ids.length ? ids.sort((a, b) => a - b)[0] : null;
    }
    if (selected_id === String(id)) {
      selected_id = null;
    }
    folded.delete(String(id));
    if (balloon_at?.id === String(id)) {
      hideBalloon();
    }
    markStart();
    setNodeButtons();
    refreshLinks();
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
  els.enabled.addEventListener('change', regenerate);
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
    refreshLinks();
    regenerate();
  });
  els.start.addEventListener('click', () => {
    start_id = selected_id;
    markStart();
    setNodeButtons();
    regenerate();
  });
  els.remove.addEventListener('click', () => {
    if (selected_id !== null) {
      editor.removeNodeId(`node-${selected_id}`);
    }
  });
  els.relayout.addEventListener('click', relayout);
  for (const event of ['mousedown', 'touchstart', 'wheel', 'keydown']) {
    els.canvas.addEventListener(
      event,
      () => {
        pristine = false;
      },
      {capture: true, passive: true},
    );
  }
  els.new_btn.addEventListener('click', () => {
    loadWorkflow('my_workflow', {on: meta.triggers[0], nodes: []});
    renderList(null);
  });
  els.copy.addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(current_snippet);
      els.copy.lastChild.textContent = ' Copied!';
    } catch {
      setYamlOpen(true);
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
  els.yaml_toggle.addEventListener('click', () =>
    setYamlOpen(els.yaml_body.hidden),
  );
}

function setYamlOpen(open) {
  els.yaml_body.hidden = !open;
  els.yaml_toggle.setAttribute('aria-expanded', String(open));
  els.yaml_toggle.firstElementChild.textContent = open
    ? 'expand_more'
    : 'expand_less';
}

async function init() {
  editor = new Drawflow(els.canvas);
  editor.reroute = false;
  // Clicking into a card's form edits it instead of dragging the card.
  editor.draggable_inputs = false;
  // Links going back left (a row's last card to the next row's first, loops)
  // run square through the gap above the target's row instead of curving
  // across the cards. 44 = from the dot (at the header) to mid-gap.
  const curve = editor.createCurvature;
  editor.createCurvature = (sx, sy, ex, ey, curvature, type) =>
    ex >= sx
      ? curve.call(editor, sx, sy, ex, ey, curvature, type)
      : ` M ${sx} ${sy} H ${sx + 20} V ${ey - 44} H ${ex - 20} V ${ey} H ${ex}`;
  // The canvas shrinks when the YAML opens: the balloon follows.
  resizes.observe(els.canvas);
  editor.start();
  // Drawflow drags a card from anything but a field (a form's scrollbar,
  // labels and padding, the header buttons): those still select the card
  // but don't move it. Runs after Drawflow's own mousedown handler.
  for (const event of ['mousedown', 'touchstart']) {
    els.canvas.addEventListener(event, e => {
      if (e.target.closest('.wf-node-form, .wf-node-btn')) {
        editor.drag = false;
      }
    });
  }
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
  // The layout measures the cards: icon ligatures must have their font.
  await document.fonts.ready;
  const first = library.find(item => item.name === 'default') ?? library[0];
  if (first) {
    loadWorkflow(first.name, first.workflow);
  }
  renderList(first?.name ?? null);
}

init();
