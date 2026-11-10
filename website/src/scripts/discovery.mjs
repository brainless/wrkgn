import { FILTERS, matchingProjects, optionsFrom, pageCount, readState, stateUrl } from '../lib/discovery-state.mjs';
import { lookingForLabel, projectTitle, sharedDate, categoryLabel } from '../lib/display.mjs';

function element(tag, className, value) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (value !== undefined) node.textContent = String(value);
  return node;
}

function appendMeta(container, label, value) {
  if (!value) return;
  const paragraph = element('p');
  paragraph.append(element('span', 'meta-label', label), document.createTextNode(` ${value}`));
  container.append(paragraph);
}

function card(project, index, categories) {
  const article = element('article', 'project-card');
  const top = element('div', 'project-card__top');
  top.append(element('span', 'project-card__index', String(index).padStart(2, '0')));
  const time = element('time', '', sharedDate(project.latest_shared_at));
  time.dateTime = new Date(project.latest_shared_at * 1000).toISOString();
  top.append(time);
  const body = element('div', 'project-card__body');
  const summary = element('div');
  const heading = element('h3');
  const link = element('a', '', projectTitle(project));
  link.href = `/projects/${project.slug}/`;
  link.dataset.projectLink = '';
  link.append(element('span', 'card-arrow', '↗'));
  link.lastElementChild.setAttribute('aria-hidden', 'true');
  heading.append(link);
  summary.append(heading, element('p', 'project-card__description', project.description));
  const meta = element('div', 'project-card__meta');
  appendMeta(meta, 'For', project.audience);
  appendMeta(meta, 'Shared by', project.builder_usernames.map((name) => `@${name}`).join(', '));
  body.append(summary, meta);
  article.append(top, body);
  if (project.categories.length || project.looking_for.length) {
    const tags = element('div', 'project-card__tags');
    for (const category of project.categories) tags.append(element('span', 'tag', categoryLabel(category, categories)));
    for (const value of project.looking_for) tags.append(element('span', 'tag tag--invitation', `${lookingForLabel(value)} · when shared`));
    article.append(tags);
  }
  return article;
}

export async function initDiscovery() {
  const root = document.querySelector('[data-directory]');
  if (!root) return;
  const version = root.dataset.version;
  const categories = JSON.parse(root.dataset.categories);
  const pageSize = Number(root.dataset.pageSize);
  const controls = root.querySelector('[data-controls]');
  const search = root.querySelector('#project-search');
  const results = root.querySelector('[data-results]');
  const pagination = root.querySelector('[data-pagination]');
  const status = root.querySelector('[data-result-count]');
  const dialog = document.querySelector('[data-project-modal]');
  const modalContent = dialog.querySelector('[data-modal-content]');
  const modalClose = dialog.querySelector('[data-modal-close]');
  let projects;
  try {
    const response = await fetch(`/data/${version}/discovery.json`);
    if (!response.ok) throw new Error('discovery unavailable');
    projects = await response.json();
    if (!Array.isArray(projects)) throw new Error('invalid discovery');
  } catch {
    // The server-rendered list and ordinary links remain available.
    return;
  }
  const bySlug = new Map(projects.map((project) => [project.slug, project]));
  const options = optionsFrom(projects);
  const detailCache = new Map();
  let state = readState(location.href, options);
  let activeModal = null;
  let modalRequest = 0;

  for (const [name, values] of Object.entries(options)) {
    const box = root.querySelector(`[data-filter="${name}"] .filter-options`);
    if (!values.length) { box.append(element('p', 'no-options', 'No values yet')); continue; }
    values.forEach((value, index) => {
      const label = element('label', 'filter-choice');
      const input = element('input');
      input.type = 'checkbox'; input.value = value; input.dataset.filterInput = name;
      input.id = `filter-${name}-${index}`;
      label.append(input, document.createTextNode(name === 'looking' ? lookingForLabel(value) : name === 'category' ? categoryLabel(value, categories) : value));
      box.append(label);
    });
  }
  controls.hidden = false;

  function syncControls() {
    if (search.value !== state.q) search.value = state.q;
    for (const input of root.querySelectorAll('[data-filter-input]')) input.checked = state[input.dataset.filterInput].includes(input.value);
  }

  function render() {
    const matches = matchingProjects(projects, state);
    state.page = Math.min(state.page, pageCount(matches, pageSize));
    const canonical = stateUrl(state);
    if (location.pathname + location.search !== canonical && !dialog.open) history.replaceState(history.state, '', canonical);
    syncControls();
    const start = (state.page - 1) * pageSize;
    const visible = matches.slice(start, start + pageSize);
    status.textContent = matches.length ? `Showing ${start + 1}–${start + visible.length} of ${matches.length} matching ${matches.length === 1 ? 'project' : 'projects'}` : 'No matching projects';
    if (!projects.length) {
      const empty = element('div', 'empty-state');
      empty.append(element('h3', '', 'Nothing on the shelf yet.'), element('p', '', 'Approved projects will appear here after the first public snapshot is staged.'));
      results.replaceChildren(empty);
    } else if (!matches.length) {
      const empty = element('div', 'empty-state');
      empty.append(element('h3', '', 'No projects match those criteria.'), element('p', '', 'Try a broader search or clear a filter.'));
      results.replaceChildren(empty);
    } else {
      const list = element('div', 'project-list');
      visible.forEach((project, index) => list.append(card(project, start + index + 1, categories)));
      results.replaceChildren(list);
    }
    pagination.replaceChildren();
    const count = pageCount(matches, pageSize);
    if (count > 1) {
      const nav = element('nav', 'pagination'); nav.setAttribute('aria-label', 'Project pages');
      const previous = element('a', '', '← Previous');
      previous.href = stateUrl({ ...state, page: Math.max(1, state.page - 1) }); previous.dataset.pageLink = String(state.page - 1);
      if (state.page === 1) { previous.className = 'pagination__disabled'; previous.setAttribute('aria-disabled', 'true'); previous.removeAttribute('href'); }
      const next = element('a', '', 'Next →');
      next.href = stateUrl({ ...state, page: Math.min(count, state.page + 1) }); next.dataset.pageLink = String(state.page + 1);
      if (state.page === count) { next.className = 'pagination__disabled'; next.setAttribute('aria-disabled', 'true'); next.removeAttribute('href'); }
      nav.append(previous, element('span', '', `Page ${state.page} of ${count}`), next);
      pagination.append(nav);
    }
  }

  function update(next) {
    state = next;
    history.pushState(null, '', stateUrl(state));
    render();
  }

  search.addEventListener('input', () => update({ ...state, q: search.value.trim(), page: 1 }));
  controls.addEventListener('change', (event) => {
    const input = event.target.closest('[data-filter-input]');
    if (!input) return;
    const name = input.dataset.filterInput;
    const selected = [...root.querySelectorAll(`[data-filter-input="${name}"]:checked`)].map((item) => item.value).sort((a, b) => a.localeCompare(b));
    update({ ...state, [name]: selected, page: 1 });
  });
  root.querySelector('[data-clear]').addEventListener('click', () => update({ q: '', category: [], audience: [], looking: [], page: 1 }));

  root.addEventListener('click', (event) => {
    const link = event.target.closest('[data-page-link]');
    if (!link || event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return;
    event.preventDefault();
    update({ ...state, page: Number(link.dataset.pageLink) });
    root.scrollIntoView({ block: 'start' });
  });

  async function fetchDetail(slug) {
    if (detailCache.has(slug)) return detailCache.get(slug);
    const project = bySlug.get(slug);
    if (!project) throw new Error('unknown project');
    const [json, html] = await Promise.all([
      fetch(`/data/${version}/projects/${slug}.json`),
      fetch(`/projects/${slug}/`),
    ]);
    if (!json.ok || !html.ok) throw new Error('project details unavailable');
    const [detail, page] = await Promise.all([json.json(), html.text()]);
    if (detail.id !== project.id || detail.slug !== slug) throw new Error('project detail mismatch');
    const parsed = new DOMParser().parseFromString(page, 'text/html');
    const source = parsed.querySelector(`.detail-page[data-public-version="${version}"] .project-detail`);
    if (!source) throw new Error('project HTML snapshot mismatch');
    detailCache.set(slug, source);
    return source;
  }

  async function openModal(slug, returnState, push) {
    const request = ++modalRequest;
    try {
      const content = await fetchDetail(slug);
      if (request !== modalRequest) return;
      modalContent.replaceChildren(document.importNode(content, true));
      if (push) history.pushState({ wrkgnModal: true, slug, ...returnState }, '', `/projects/${slug}/`);
      activeModal = { slug, ...returnState };
      if (!dialog.open) dialog.showModal();
      modalClose.focus();
    } catch {
      location.assign(`/projects/${slug}/`);
    }
  }

  root.addEventListener('click', (event) => {
    const link = event.target.closest('[data-project-link]');
    if (!link || event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey || link.target || link.hasAttribute('download')) return;
    const slug = link.pathname.match(/^\/projects\/([^/]+)\/$/)?.[1];
    if (!slug || !bySlug.has(slug)) return;
    event.preventDefault();
    openModal(slug, { returnUrl: location.pathname + location.search, scrollY: scrollY, focusSlug: slug }, true);
  });

  function closeModal() { if (dialog.open) history.back(); }
  modalClose.addEventListener('click', closeModal);
  dialog.addEventListener('cancel', (event) => { event.preventDefault(); closeModal(); });
  dialog.addEventListener('click', (event) => { if (event.target === dialog) closeModal(); });
  window.addEventListener('popstate', () => {
    const modalState = history.state;
    if (modalState?.wrkgnModal && bySlug.has(modalState.slug)) {
      openModal(modalState.slug, modalState, false);
      return;
    }
    ++modalRequest;
    const prior = activeModal;
    activeModal = null;
    if (dialog.open) dialog.close();
    state = readState(location.href, options);
    render();
    if (prior) {
      scrollTo(0, prior.scrollY);
      root.querySelector(`[data-project-link][href="/projects/${prior.focusSlug}/"]`)?.focus();
    }
  });
  render();
}
