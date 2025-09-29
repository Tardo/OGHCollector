// Copyright 2025 Alexandre D. Díaz
import {Graph} from 'graphology';
import {random} from 'graphology-layout';
import FA2Layout from 'graphology-layout-forceatlas2/worker';
import forceAtlas2 from 'graphology-layout-forceatlas2';
import Sigma from 'sigma';
import {Component, getService, registerComponent, HTTP_METHOD} from 'mirlo';
import '@scss/components/atlas.scss';

class SigmaAtlas extends Component {
  DEFAULT_LAYOUT_TIMEOUT = 8000;
  #sigma_renderer = null;
  #fa2_layout = null;
  #timer = null;
  #el_loading_msg = null;
  #el_search_select_ver = null;
  #el_mod_dep_graph = null;
  #el_mod_dep_control = null;
  #el_mod_dep_search_dependencies = null;

  onSetup() {
    Component.useStyles('/static/auto/web/scss/components/atlas.css');
    Component.disableShadow();
    Component.useEvents({
      sigma_atlas_mod_dep_search_input: {
        mode: 'id',
        events: {
          input: this.onInputSearchInput,
        },
      },
      sigma_atlas_mod_dep_search_select_ver: {
        mode: 'id',
        events: {
          change: this.onChangeSearchVersion,
        },
      },
      sigma_atlas_mod_dep_control: {
        mode: 'id',
        events: {
          click: this.onClickControl,
        },
      },
    });
    Component.useFetchData({
      odoo_versions: {
        endpoint: '/common/odoo/versions',
        method: HTTP_METHOD.GET,
      },
    });
  }

  async onWillStart() {
    await super.onWillStart(...arguments);
    this.#el_loading_msg = this.queryId(
      'sigma_atlas_mod_dep_graph_loading_msg',
    );
    this.#el_search_select_ver = this.queryId(
      'sigma_atlas_mod_dep_search_select_ver',
    );
    this.#el_mod_dep_graph = this.queryId('sigma_atlas_mod_dep_graph');
    this.#el_mod_dep_control = this.queryId('sigma_atlas_mod_dep_control');
    this.#el_mod_dep_search_dependencies = this.queryId(
      'sigma_atlas_mod_dep_search_dependencies',
    );
    this.#update();
  }

  onStart() {
    super.onStart();
    this.#fillOdooVersionsSearchOptions();
  }

  onRemove() {
    this.killGraph();
  }

  killGraph() {
    if (this.#timer) {
      clearTimeout(this.#timer);
      this.#timer = null;
    }
    if (this.#sigma_renderer) {
      this.#fa2_layout.kill();
      this.#fa2_layout = null;
      this.#sigma_renderer.graph.clear();
      this.#sigma_renderer.kill();
      this.#sigma_renderer = null;
    }
  }

  toggleLoadingMessage(visible) {
    if (visible) {
      this.#el_loading_msg.style.display = '';
    } else {
      this.#el_loading_msg.style.display = 'none';
    }
  }

  async #update() {
    this.toggleLoadingMessage(true);

    this.mirlo.state.odoo_version =
      this.#el_search_select_ver.value ||
      this.getFetchData('odoo_versions')[0].value;
    const data = await getService('requests').getJSON(
      `/atlas/data/${this.mirlo.state.odoo_version}`,
    );

    // FIXME: Due to incosistences with forceAtlas need reconstruct the graph node.
    this.killGraph();

    // Create the forceAtlas graph
    const graph = Graph.from(data);
    random.assign(graph);

    this.#sigma_renderer = new Sigma(graph, this.#el_mod_dep_graph, {
      labelColor: {color: '#763626'},
      nodeReducer: this.#nodeReducer.bind(this),
      edgeReducer: this.#edgeReducer.bind(this),
      minArrowSize: 2,
      defaultEdgeType: 'arrow',
      arrowSizeRatio: 10,
      defaultEdgeColor: 'red',
      defaultNodeColor: '#007FFF',
      hideEdgesOnMove: false,
      allowInvalidContainer: true,
    });
    // Bind graph interactions
    this.#sigma_renderer.on('clickNode', ({node}) => {
      if (this.mirlo.state.hoveredNode === node) {
        this.setHoveredNode(undefined);
      } else {
        this.setHoveredNode(node);
      }
    });
    // this.#sigma_renderer.on('enterNode', ({node}) => {
    //   this.setHoveredNode(node);
    // });
    // this.#sigma_renderer.on('leaveNode', () => {
    //   this.setHoveredNode(undefined);
    // });
    // Start forceAtlas2 worker
    const sensibleSettings = forceAtlas2.inferSettings(graph);
    sensibleSettings.gravity = 0.5;
    this.#fa2_layout = new FA2Layout(graph, {
      settings: sensibleSettings,
    });
    this.#fa2_layout.start();
    this.#fillDependecySearchOptions();
    this.#el_mod_dep_control.textContent = '⏹️';

    if (!this.#timer) {
      this.#timer = setTimeout(
        this.onPauseAtlasLayout.bind(this),
        this.DEFAULT_LAYOUT_TIMEOUT,
      );
    }

    this.toggleLoadingMessage(false);
  }

  #fillDependecySearchOptions() {
    this.#el_mod_dep_search_dependencies.replaceChildren();
    this.#sigma_renderer.graph
      .nodes()
      .map(
        n =>
          new Option(
            this.#sigma_renderer.graph.getNodeAttribute(n, 'label'),
            n,
          ),
      )
      .forEach(option =>
        this.#el_mod_dep_search_dependencies.appendChild(option),
      );
  }

  #fillOdooVersionsSearchOptions() {
    this.#el_search_select_ver.replaceChildren();
    this.getFetchData('odoo_versions')
      .map(({value}) => new Option(value))
      .forEach(option => this.#el_search_select_ver.add(option));
  }

  setHoveredNode(node) {
    if (node) {
      this.mirlo.state.hoveredNode = node;
      this.mirlo.state.hoveredNeighbors = new Set(
        this.#sigma_renderer.graph.neighbors(node),
      );
    } else {
      this.mirlo.state.hoveredNode = undefined;
      this.mirlo.state.hoveredNeighbors = undefined;
    }

    this.#sigma_renderer.refresh();
  }

  #nodeReducer(node, data) {
    const res = {...data};

    if (
      this.mirlo.state.hoveredNeighbors &&
      !this.mirlo.state.hoveredNeighbors.has(node) &&
      this.mirlo.state.hoveredNode !== node
    ) {
      res.hidden = true;
      // res.label = '';
      // res.color = '#242c38';
    }

    if (this.mirlo.state.selectedNode === node) {
      res.highlighted = true;
    } else if (
      this.mirlo.state.suggestions &&
      !this.mirlo.state.suggestions.has(node)
    ) {
      // res.label = '';
      // res.color = '#242c38';
      res.hidden = true;
    }

    return res;
  }

  #edgeReducer(edge, data) {
    const res = {...data};

    if (
      this.mirlo.state.hoveredNode &&
      !this.#sigma_renderer.graph.hasExtremity(
        edge,
        this.mirlo.state.hoveredNode,
      )
    ) {
      res.hidden = true;
    }

    if (
      this.mirlo.state.suggestions &&
      (!this.mirlo.state.suggestions.has(
        this.#sigma_renderer.graph.source(edge),
      ) ||
        !this.mirlo.state.suggestions.has(
          this.#sigma_renderer.graph.target(edge),
        ))
    ) {
      res.hidden = true;
    }

    return res;
  }

  onPauseAtlasLayout() {
    this.#fa2_layout.stop();
    this.#el_mod_dep_control.textContent = '▶️';
  }

  onClickControl() {
    const running = this.#fa2_layout.isRunning();
    if (running) {
      if (this.#timer) {
        clearTimeout(this.#timer);
        this.#timer = null;
      }
      this.#fa2_layout.stop();
      this.#el_mod_dep_control.textContent = '▶️';
    } else {
      this.#fa2_layout.start();
      this.#el_mod_dep_control.textContent = '⏹️';
    }
  }

  onInputSearchInput(ev) {
    const query = ev.target.value;
    let has_match = false;
    if (query) {
      const suggestions = this.#sigma_renderer.graph
        .nodes()
        .filter(node_id => node_id.includes(query));
      if (suggestions.length !== 0) {
        const exact_match = suggestions.indexOf(query);
        if (exact_match !== -1) {
          this.mirlo.state.selectedNode = suggestions[exact_match];
          this.mirlo.state.suggestions = undefined;

          const nodePosition = this.#sigma_renderer.getNodeDisplayData(
            this.mirlo.state.selectedNode,
          );
          this.#sigma_renderer.getCamera().animate(nodePosition, {
            duration: 500,
          });
        } else {
          this.mirlo.state.selectedNode = undefined;
          this.mirlo.state.suggestions = new Set(suggestions);
        }
        has_match = true;
      }
    }
    if (!has_match) {
      this.mirlo.state.selectedNode = undefined;
      this.mirlo.state.suggestions = undefined;
    }
    this.#sigma_renderer.refresh();
  }

  onChangeSearchVersion(ev) {
    if (ev.target.value !== this.mirlo.state.odoo_version) {
      this.#update();
    }
  }
}

registerComponent('atlas', SigmaAtlas);
