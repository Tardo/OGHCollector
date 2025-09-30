// Copyright 2025 Alexandre D. Díaz
import {AnimatedComponent, registerComponent, HTTP_METHOD} from 'mirlo';
import '@scss/components/module-counter.scss';

class ModuleCounter extends AnimatedComponent {
  static observedAttributes = ['version'];

  #INC_STEP = 20;

  #el_main_container = null;
  #el_version = null;
  #el_counter_org = null;
  #el_contrib_rank = null;
  #el_committer_rank = null;
  #module_count = 0;

  onSetup() {
    AnimatedComponent.useStateBinds({
      mod_counter: {
        id: 'counter',
      },
    });
    AnimatedComponent.useEvents({
      version: {
        mode: 'id',
        events: {
          change: this.onChangeVersion,
        },
      },
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
      odoo_versions: {
        endpoint: '/common/odoo/versions',
        method: HTTP_METHOD.GET,
      },
    });
  }

  async onWillStart() {
    await super.onWillStart(...arguments);

    this.#el_main_container = this.queryId('main_container');
    this.#el_version = this.queryId('version');
    this.#el_counter_org = this.queryId('counter_org');
    this.#el_contrib_rank = this.queryId('contrib_rank');
    this.#el_committer_rank = this.queryId('committer_rank');
  }

  onStart() {
    super.onStart();

    this.#render(this.mirlo.options.version);
  }

  onAnimationStep() {
    if (this.mirlo.state.mod_counter === this.#module_count) {
      this.stopAnimation();
      this.#el_main_container.classList.remove('changing');
    } else if (this.mirlo.state.mod_counter < this.#module_count) {
      this.mirlo.state.mod_counter += this.#INC_STEP;
    } else if (this.mirlo.state.mod_counter > this.#module_count) {
      this.mirlo.state.mod_counter = this.#module_count;
    }
  }

  onChangeVersion(ev) {
    this.#el_main_container.classList.remove('changing');
    this.#el_main_container.classList.add('changing');
    this.#render(ev.target.value);
  }

  #render(odoo_version) {
    this.#fillOdooVersionsSearchOptions(odoo_version);
    const count_info = this.getFetchData('count_info');
    const version_count_infos = count_info.filter(
      item => item.version === odoo_version,
    );
    const ul_el = document.createElement('ol');
    this.#module_count = 0;
    for (const index in version_count_infos) {
      const version_count_info = version_count_infos[index];
      this.#module_count += version_count_info.count;
      const li_el = document.createElement('li');
      li_el.textContent = `${version_count_info.org.toUpperCase()}: ${version_count_info.count}`;
      ul_el.appendChild(li_el);
    }
    this.#el_counter_org.replaceChildren(ul_el);

    const contrib_info = this.getFetchData('rank_info');
    const contrib_rank_infos = contrib_info.filter(
      item => item.version === odoo_version,
    );
    const contrib_rank_ul_el = document.createElement('ol');
    for (const index in contrib_rank_infos) {
      const contrib_rank_info = contrib_rank_infos[index];
      const li_el = document.createElement('li');
      li_el.textContent = `${contrib_rank_info.contrib.toUpperCase()}: ${contrib_rank_info.count}`;
      contrib_rank_ul_el.appendChild(li_el);
    }
    this.#el_contrib_rank.replaceChildren(contrib_rank_ul_el);

    const committer_info = this.getFetchData('committer_info');
    const committer_rank_infos = committer_info.filter(
      item => item.version === odoo_version,
    );
    const committer_rank_ul_el = document.createElement('ol');
    for (const index in committer_rank_infos) {
      const committer_rank_info = committer_rank_infos[index];
      const li_el = document.createElement('li');
      li_el.textContent = `${committer_rank_info.committer.toUpperCase()}: ${committer_rank_info.count}`;
      committer_rank_ul_el.appendChild(li_el);
    }
    this.#el_committer_rank.replaceChildren(committer_rank_ul_el);

    this.mirlo.state.mod_counter = 0;
    this.startAnimation();
  }

  #fillOdooVersionsSearchOptions(selected_value) {
    this.#el_version.replaceChildren();
    this.getFetchData('odoo_versions')
      .map(({value}) => new Option(value))
      .forEach(option => {
        if (option.value === selected_value) {
          option.selected = 'selected';
        }
        this.#el_version.add(option);
      });
  }
}

registerComponent('module-counter', ModuleCounter);
