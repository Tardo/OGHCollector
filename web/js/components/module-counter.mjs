// Copyright 2025 Alexandre D. Díaz
import {AnimatedComponent, registerComponent, HTTP_METHOD} from 'mirlo';
import '@scss/components/module-counter.scss';


class ModuleCounter extends AnimatedComponent {
  static observedAttributes = ['version'];

  #INC_STEP = 20;

  #version = null;
  #counter = null;
  #counter_org = null;
  #contrib_rank = null;
  #committer_rank = null;
  #module_count = 0;

  onSetup() {
    AnimatedComponent.useStyles('/static/auto/web/scss/components/module-counter.css');
    AnimatedComponent.useStateBinds({
      mod_counter: {
        id: 'counter',
      }
    });
    AnimatedComponent.useFetchData({
      count_info: {
        endpoint: '/common/odoo/module/count',
        method: HTTP_METHOD.GET,
        cache_name: 'modules-count',
      },
      rank_info: {
        endpoint: '/common/odoo/contributor/rank',
        method: HTTP_METHOD.GET,
        cache_name: 'contributor-rank',
      },
      committer_info: {
        endpoint: '/common/odoo/committer/rank',
        method: HTTP_METHOD.GET,
        cache_name: 'committer-rank',
      },
    });
  }

  onStart() {
    super.onStart();
    this.#version = this.queryId('version');
    this.#counter = this.queryId('counter');
    this.#counter_org = this.queryId('counter_org');
    this.#contrib_rank = this.queryId('contrib_rank');
    this.#committer_rank = this.queryId('committer_rank');

    const count_info = this.getFetchData('count_info');
    const version_count_infos = count_info.filter(item => item.version === this.mirlo.options.version);
    const ul_el = document.createElement('ol');
    for (const index in version_count_infos) {
      const version_count_info = version_count_infos[index];
      this.#module_count += version_count_info.count;
      const li_el = document.createElement('li');
      li_el.textContent = `${version_count_info.org.toUpperCase()}: ${version_count_info.count}`;
      ul_el.appendChild(li_el);
    }
    this.#counter_org.replaceChildren(ul_el);

    const contrib_info = this.getFetchData('rank_info');
    const contrib_rank_infos = contrib_info.filter(item => item.version === this.mirlo.options.version);
    const contrib_rank_ul_el = document.createElement('ol');
    for (const index in contrib_rank_infos) {
      const contrib_rank_info = contrib_rank_infos[index];
      const li_el = document.createElement('li');
      li_el.textContent = `${contrib_rank_info.contrib.toUpperCase()}: ${contrib_rank_info.count}`;
      contrib_rank_ul_el.appendChild(li_el);
    }
    this.#contrib_rank.replaceChildren(contrib_rank_ul_el);

    const committer_info = this.getFetchData('committer_info');
    const committer_rank_infos = committer_info.filter(item => item.version === this.mirlo.options.version);
    const committer_rank_ul_el = document.createElement('ol');
    for (const index in committer_rank_infos) {
      const committer_rank_info = committer_rank_infos[index];
      const li_el = document.createElement('li');
      li_el.textContent = `${committer_rank_info.committer.toUpperCase()}: ${committer_rank_info.count}`;
      committer_rank_ul_el.appendChild(li_el);
    }
    this.#committer_rank.replaceChildren(committer_rank_ul_el);

    this.mirlo.state.mod_counter = 0;
    this.#version.textContent = this.mirlo.options.version;
  }

  onAnimationStep() {
    if (this.mirlo.state.mod_counter === this.#module_count) {
      this.stopAnimation();
    } else if (this.mirlo.state.mod_counter < this.#module_count) {
      this.mirlo.state.mod_counter += this.#INC_STEP;
    } else if (this.mirlo.state.mod_counter > this.#module_count) {
      this.mirlo.state.mod_counter = this.#module_count;
    }
  }
}

registerComponent('module-counter', ModuleCounter);