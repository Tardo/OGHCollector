// The globe page (/localization): the globe, the version filter, and the
// chosen country's modules listed in place below it. Country links keep
// their /localization/{code} href (new tab, no JS, sitemap).
import {geoContains, geoGraticule10, geoOrthographic, geoPath} from 'd3-geo';
import {feature} from 'topojson-client';
import {whereAlpha2, whereNumeric} from 'iso-3166-1';
import world from 'world-atlas/countries-50m.json';

const canvas = document.querySelector('#localization-globe');
if (canvas) {
  const ctx = canvas.getContext('2d');
  const stage = canvas.closest('.globe-stage');
  const caption = document.querySelector('#globe-caption');
  const versionSelect = document.querySelector('#localization-version');
  const countryList = document.querySelector('.localization-countries');
  const selection = document.querySelector('#localization-selection');
  const selectionTitle = document.querySelector(
    '#localization-selection-title',
  );
  const selectionCount = document.querySelector(
    '#localization-selection-count',
  );
  const selectionList = document.querySelector('#localization-selection-list');
  const links = new Map();
  const countryLinks = [];
  const countries = feature(world, world.objects.countries).features;
  const codes = new Map(
    countries.map(country => [whereNumeric(country.id)?.alpha2, country.id]),
  );
  for (const link of document.querySelectorAll('.localization-countries a')) {
    const country = whereAlpha2(
      link.dataset.country === 'UK' ? 'GB' : link.dataset.country,
    );
    // Regional codes such as EU are not countries and have no place on a globe.
    if (!country) {
      link.hidden = true;
      continue;
    }
    link.querySelector('.country-name').textContent = country.country;
    // Not every country is on the atlas: those are only in the list.
    countryLinks.push({id: codes.get(country.alpha2), link});
  }

  const requestedVersion = new URL(location.href).searchParams.get('version');
  const requestedCountry = new URL(location.href).searchParams.get('country');
  if (
    [...versionSelect.options].some(option => option.value === requestedVersion)
  ) {
    versionSelect.value = requestedVersion;
  }

  function filterCountries() {
    links.clear();
    for (const {id, link} of countryLinks) {
      const matches =
        Number(link.dataset.version) === Number(versionSelect.value) * 10;
      link.hidden = !matches;
      if (matches) {
        link.href = `/localization/${link.dataset.country.toLowerCase()}?version=${encodeURIComponent(versionSelect.value)}`;
        if (id) links.set(id, link);
      }
    }
    showCountry(null);
    draw();
  }

  const projection = geoOrthographic().clipAngle(90).precision(1);
  const path = geoPath(projection, ctx);
  const grid = geoGraticule10();
  const reduceMotion = matchMedia('(prefers-reduced-motion: reduce)');
  let width = 0;
  let height = 0;
  let radius = 0;
  let rotation = [-16, -17];
  let hovering = false;
  let visible = true;
  let dragging = null;
  let active = null;
  let selected = null;
  let request = null;
  let lastFrame = 0;

  function draw() {
    ctx.clearRect(0, 0, width, height);
    if (!radius) return;
    projection
      .translate([width / 2, height / 2])
      .scale(radius)
      .rotate(rotation);

    const ocean = ctx.createRadialGradient(
      width * 0.39,
      height * 0.33,
      radius * 0.08,
      width / 2,
      height / 2,
      radius,
    );
    ocean.addColorStop(0, '#1b536b');
    ocean.addColorStop(0.72, '#0b283a');
    ocean.addColorStop(1, '#06131f');
    ctx.beginPath();
    path({type: 'Sphere'});
    ctx.fillStyle = ocean;
    ctx.fill();
    ctx.strokeStyle = 'rgba(77, 192, 210, 0.48)';
    ctx.lineWidth = 1.5;
    ctx.stroke();

    ctx.beginPath();
    path(grid);
    ctx.strokeStyle = 'rgba(118, 182, 196, 0.13)';
    ctx.lineWidth = 0.7;
    ctx.stroke();

    for (const country of countries) {
      const enabled = links.has(country.id);
      ctx.beginPath();
      path(country);
      ctx.fillStyle = enabled
        ? country === active || links.get(country.id) === selected
          ? '#7af5da'
          : '#2fbaa5'
        : 'rgba(107, 147, 156, 0.23)';
      ctx.fill();
      ctx.strokeStyle = enabled
        ? 'rgba(154, 255, 225, 0.85)'
        : 'rgba(155, 195, 199, 0.37)';
      ctx.lineWidth = enabled ? 0.9 : 0.55;
      ctx.stroke();
    }
  }

  function countryAt(event) {
    const rect = canvas.getBoundingClientRect();
    const x = event.clientX - rect.left;
    const y = event.clientY - rect.top;
    if (Math.hypot(x - width / 2, y - height / 2) > radius) return null;
    const coordinate = projection.invert([x, y]);
    return (
      countries.find(
        country => links.has(country.id) && geoContains(country, coordinate),
      ) || null
    );
  }

  function showCountry(country) {
    if (!dragging && active === country) return;
    active = country;
    const link = country && links.get(country.id);
    caption.textContent = link
      ? `${link.querySelector('.country-name').textContent} · ${link.querySelector('.country-count').firstChild.textContent.trim()} modules · Click to list them`
      : 'Hover to pause · Drag to rotate · Select a highlighted country';
    canvas.style.cursor = link ? 'pointer' : 'grab';
    draw();
  }

  // Lists the country's modules below the globe (null = none), and keeps
  // both choices in the URL so it can be shared. `reveal`: scroll to the list
  // once loaded if it starts off-screen (stacked layout, short windows).
  async function select(link, reveal = false) {
    selected?.removeAttribute('aria-current');
    selected = link;
    request?.abort();
    const url = new URL(location.href);
    url.searchParams.set('version', versionSelect.value);
    if (link)
      url.searchParams.set('country', link.dataset.country.toLowerCase());
    else url.searchParams.delete('country');
    history.replaceState(null, '', url);
    draw();
    selection.hidden = !link;
    if (!link) return;
    link.setAttribute('aria-current', 'true');
    selectionTitle.textContent = `${link.querySelector('.country-name').textContent} · Odoo ${versionSelect.value}`;
    selectionCount.textContent = 'Loading…';
    request = new AbortController();
    try {
      const res = await fetch(
        `/localization/${link.dataset.country.toLowerCase()}/modules?version=${encodeURIComponent(versionSelect.value)}`,
        {signal: request.signal},
      );
      if (!res.ok) throw new Error(res.statusText);
      selectionList.innerHTML = await res.text();
      const count = selectionList.querySelectorAll(
        '.localization-result',
      ).length;
      selectionCount.textContent = `${count} module${count === 1 ? '' : 's'}`;
      if (
        reveal &&
        selectionList.getBoundingClientRect().top > innerHeight - 80
      ) {
        selection.scrollIntoView({
          behavior: reduceMotion.matches ? 'auto' : 'smooth',
          block: 'start',
        });
      }
    } catch (error) {
      if (error.name === 'AbortError') return;
      selectionList.replaceChildren();
      selectionCount.textContent = 'Could not load the modules.';
    }
  }

  filterCountries();
  const initial = countryList.querySelector(
    `a[data-country="${CSS.escape((requestedCountry || '').toUpperCase())}"]:not([hidden])`,
  );
  if (initial) select(initial);
  versionSelect.addEventListener('change', () => {
    filterCountries();
    select(selected && !selected.hidden ? selected : null);
  });
  countryList.addEventListener('click', event => {
    const link = event.target.closest('a');
    // Modified clicks still open the country's own page.
    if (
      !link ||
      event.button ||
      event.ctrlKey ||
      event.metaKey ||
      event.shiftKey ||
      event.altKey
    ) {
      return;
    }
    event.preventDefault();
    select(link, true);
  });

  canvas.addEventListener('pointerenter', () => {
    hovering = true;
  });
  canvas.addEventListener('pointerleave', () => {
    hovering = false;
    if (!dragging) showCountry(null);
  });
  canvas.addEventListener('pointerdown', event => {
    dragging = {x: event.clientX, y: event.clientY, moved: false};
    canvas.setPointerCapture(event.pointerId);
  });
  canvas.addEventListener('pointermove', event => {
    if (dragging) {
      const dx = event.clientX - dragging.x;
      const dy = event.clientY - dragging.y;
      if (Math.abs(dx) + Math.abs(dy) > 3) dragging.moved = true;
      rotation = [
        rotation[0] + dx * 0.28,
        Math.max(-80, Math.min(80, rotation[1] - dy * 0.28)),
      ];
      dragging.x = event.clientX;
      dragging.y = event.clientY;
      showCountry(null);
    } else {
      showCountry(countryAt(event));
    }
  });
  canvas.addEventListener('pointerup', event => {
    if (dragging && !dragging.moved) {
      const country = countryAt(event);
      if (country) links.get(country.id).click();
    }
    dragging = null;
  });
  canvas.addEventListener('pointercancel', () => {
    dragging = null;
  });

  new ResizeObserver(() => {
    const rect = canvas.getBoundingClientRect();
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    width = rect.width;
    height = rect.height;
    radius = Math.min(width, height) * 0.44;
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(height * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    draw();
  }).observe(canvas);
  new IntersectionObserver(entries => {
    visible = entries[0].isIntersecting;
  }).observe(stage);

  function animate(time) {
    if (
      visible &&
      !document.hidden &&
      !hovering &&
      !dragging &&
      !reduceMotion.matches &&
      time - lastFrame > 70
    ) {
      rotation[0] += Math.min(time - lastFrame || 70, 100) * 0.004;
      lastFrame = time;
      draw();
    } else if (!visible || document.hidden || hovering || dragging) {
      lastFrame = time;
    }
    requestAnimationFrame(animate);
  }
  requestAnimationFrame(animate);
}
