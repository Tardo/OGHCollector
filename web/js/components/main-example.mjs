// Copyright (C) 2024 Alexandre Díaz
import {AnimatedComponent, registerComponent, mirloComponent} from 'mirlo';
import '@scss/components/main-example.scss';

const EXAMPLES = [
  `/v1/module/web_responsive/16.0?org=OCA&repo=web`,
  `/v1/search/sales`,
  `/v1/module/contract_sale_generation`,
  `/v1/repository/product-pack`,
  `/v1/search/account?odoo_version=14.0`,
  `/v1/module/shopfloor_base/16.0`,
  `/v1/module/project_hr/15.0?repo=project`,
];

function getRandom(min, max) {
  return min + Math.floor(Math.random() * (max - min));
}
function getRandomExampleIndex() {
  return getRandom(0, EXAMPLES.length);
}

class InputMainExample extends AnimatedComponent {
  #TIME_DELAY = 75;
  #TIME_DELAY_CHANGE = 5000;
  #next_frame_timestamp = 0;
  #selected_example = getRandomExampleIndex();
  #chars_size = 0;
  #direction = 1;
  #is_paused = false;

  onSetup() {
    AnimatedComponent.useStyles('/static/auto/web/scss/components/main-example.css');
    AnimatedComponent.useEvents({
      display: {
        mode: 'id',
        events: {
          mouseenter: this.onMouseEnter,
          mouseleave: this.onMouseLeave,
        },
      },
    });
  }

  onAnimationStep(timestamp) {
    if (!this.#is_paused && timestamp >= this.#next_frame_timestamp) {
      const example_text = EXAMPLES[this.#selected_example];
      const selected_text = example_text.substring(0, this.#chars_size);
      if (this.#direction === 1) {
        ++this.#chars_size;
      } else {
        --this.#chars_size;
      }
      this.queryId('display').value =
        `curl -s ${window.location.origin}${selected_text}`;

      if (this.#direction === 1 && this.#chars_size > example_text.length) {
        this.#next_frame_timestamp = timestamp + this.#TIME_DELAY_CHANGE;
        this.#direction = -1;
      } else if (this.#chars_size < 0 && this.#direction === -1) {
        this.#next_frame_timestamp = timestamp;
        this.#chars_size = 0;
        this.#direction = 1;
        const last_example = this.#selected_example;
        do {
          this.#selected_example = getRandomExampleIndex();
        } while (this.#selected_example === last_example);
      } else {
        if (this.#direction === -1) {
          this.#next_frame_timestamp = timestamp + 45;
        } else {
          this.#next_frame_timestamp =
            timestamp + this.#TIME_DELAY + getRandom(-30, 120);
        }
      }
    }
  }

  onMouseEnter(ev) {
    this.#is_paused = true;
  }

  onMouseLeave(ev) {
    this.#is_paused = false;
  }
}

registerComponent('main-example', InputMainExample);