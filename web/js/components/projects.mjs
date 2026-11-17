// Copyright 2026 Alexandre D. Díaz
// Doodba projects (pages/projects.html, pages/project.html): server-rendered;
// this filters the table rows by text, Odoo version and problem flags, and
// sorts them by a clicked header.
import {Component, registerComponent} from 'mirlo';
import '@scss/pages/projects.scss';
import {filterRows, filterTerm, matchesTerm} from '@app/utils/table-filter';

class Projects extends Component {
  #version = '';
  #flags = new Set();

  onSetup() {
    Component.disableShadow();
    Component.useEvents({
      projects_search: {mode: 'id', events: {input: this.applyFilter}},
      'projects-versions': {mode: 'id', events: {click: this.onClickVersion}},
      'projects-flags': {mode: 'id', events: {click: this.onClickFlag}},
      '#projects-table th[data-sort]': {
        mode: 'selector',
        events: {click: this.onClickSort},
      },
    });
  }

  onStart() {
    super.onStart();
    this.queryAll('#projects-table th[data-sort]').forEach(th => {
      th.style.cursor = 'pointer';
    });
  }

  applyFilter() {
    const term = filterTerm(this.queryId('projects_search'));
    const any_visible = filterRows(
      this.queryAll('#projects-table tbody tr'),
      row =>
        matchesTerm(row, term) &&
        (this.#version === '' || row.dataset.version === this.#version) &&
        [...this.#flags].every(f => row.dataset.flags.split(' ').includes(f)),
    );
    const no_match = this.queryId('projects-no-match');
    if (no_match) {
      no_match.hidden = any_visible;
    }
  }

  onClickVersion(ev) {
    const btn = ev.target.closest('button[data-version]');
    if (!btn) {
      return;
    }
    this.#version = btn.dataset.version;
    ev.currentTarget
      .querySelectorAll('.nav-link')
      .forEach(b => b.classList.toggle('active', b === btn));
    this.applyFilter();
  }

  onClickFlag(ev) {
    const btn = ev.target.closest('button[data-flag]');
    if (!btn) {
      return;
    }
    const on = !this.#flags.has(btn.dataset.flag);
    this.#flags[on ? 'add' : 'delete'](btn.dataset.flag);
    btn.classList.toggle('active', on);
    btn.setAttribute('aria-pressed', String(on));
    this.applyFilter();
  }

  // Click a sortable header: numeric columns descending first, text ascending.
  onClickSort(ev) {
    const th = ev.currentTarget;
    const key = th.dataset.sort;
    const text = key === 'name' || key === 'version' || key === 'last';
    const dir = th.dataset.dir === 'asc' ? -1 : 1;
    th.parentElement.querySelectorAll('th').forEach(o => delete o.dataset.dir);
    th.dataset.dir = dir === 1 ? 'asc' : 'desc';
    const body = this.query('#projects-table tbody');
    const rows = [...body.querySelectorAll('tr')];
    rows.sort((a, b) => {
      const x = a.dataset[key];
      const y = b.dataset[key];
      const c = text
        ? x.localeCompare(y, undefined, {numeric: true})
        : Number(x) - Number(y);
      return (text ? c : -c) * dir;
    });
    rows.forEach(r => body.appendChild(r));
  }
}

registerComponent('projects', Projects);
