import { normalizeQuery, readState, stateUrl, type State } from './canonical';
import { mobileFilters } from './overlay';

document.documentElement.classList.add('js');
const filters = mobileFilters();
const directory = document.querySelector<HTMLElement>('[data-directory]');
const form = document.querySelector<HTMLFormElement>('[data-controls]');
// Standalone detail pages keep ordinary server GET navigation and mobile controls.
if (directory && form) enhance(directory, form);

type Entry = { kind: 'list' | 'modal'; url: string; scroll: number; listUrl?: string; focusHref?: string };
type HistoryMode = 'push' | 'replace' | 'none';
function enhance(directory: HTMLElement, form: HTMLFormElement) {
  const dialog = document.querySelector<HTMLDialogElement>('[data-project-modal]');
  const modalContent = dialog?.querySelector<HTMLElement>('[data-modal-content]');
  let listUrl = directory.dataset.listUrl || location.pathname + location.search;
  let sequence = 0;
  let controller: AbortController | null = null;
  let searchTimer = 0;
  let searchSession = false;
  let searchEpoch = 0;
  let modalContext: Entry | null = null;
  let modalOpener: HTMLElement | null = null;
  const current = () => location.pathname + location.search;
  const searchInput = () => form.querySelector<HTMLInputElement>('[name="q"]')!;
  const initial: Entry = { kind: 'list', url: listUrl, scroll: window.scrollY };
  history.replaceState(initial, '', listUrl);
  history.scrollRestoration = 'manual';

  function rememberScroll() {
    const entry = history.state as Entry | null;
    if (entry?.kind === 'list') history.replaceState({ ...entry, scroll: window.scrollY }, '', location.href);
  }
  function begin() {
    controller?.abort();
    controller = new AbortController();
    return { id: ++sequence, signal: controller.signal };
  }
  async function fetchPage(url: string, signal: AbortSignal) {
    const response = await fetch(url, { signal });
    if (!response.ok || new URL(response.url).origin !== location.origin) throw new Error('Navigation failed');
    const page = new DOMParser().parseFromString(await response.text(), 'text/html');
    return { page, url: new URL(response.url).pathname + new URL(response.url).search };
  }
  function syncControls(page: Document) {
    const next = page.querySelector<HTMLFormElement>('[data-controls]');
    if (!next) throw new Error('Missing controls');
    const active = document.activeElement;
    const q = next.querySelector<HTMLInputElement>('[name="q"]');
    if (q && (active !== searchInput() || normalizeQuery(searchInput().value) !== q.value)) searchInput().value = q.value;
    for (const name of ['category', 'audience']) {
      const group = form.querySelector(`[data-filter="${name}"]`);
      const incoming = next.querySelector(`[data-filter="${name}"]`);
      if (!group || !incoming) continue;
      const oldInputs = [...group.querySelectorAll<HTMLInputElement>('input')];
      const newInputs = [...incoming.querySelectorAll<HTMLInputElement>('input')];
      if (oldInputs.map(node => node.value).join('\0') !== newInputs.map(node => node.value).join('\0')) {
        group.replaceWith(incoming.cloneNode(true));
      } else oldInputs.forEach((node, index) => { node.checked = newInputs[index].checked; });
    }
  }
  // Only title, canonical, robots and description are synced. Open Graph/Twitter
  // tags and JSON-LD may go stale after in-page navigation; this is accepted
  // because crawlers and link-preview bots fetch full pages.
  function syncHead(page: Document) {
    document.title = page.title;
    for (const selector of ['link[rel="canonical"]', 'meta[name="robots"]', 'meta[name="description"]']) {
      const old = document.head.querySelector(selector), next = page.head.querySelector(selector);
      if (next) old ? old.replaceWith(next.cloneNode(true)) : document.head.append(next.cloneNode(true));
      else old?.remove();
    }
  }
  async function loadList(url: string, mode: HistoryMode, scroll?: number) {
    const request = begin();
    directory.setAttribute('aria-busy', 'true');
    try {
      const { page, url: finalUrl } = await fetchPage(url, request.signal);
      if (request.id !== sequence) return false;
      const incoming = page.querySelector<HTMLElement>('[data-directory]');
      const region = incoming?.querySelector('[data-discovery-region]');
      const existing = directory.querySelector('[data-discovery-region]');
      if (!incoming || !region || !existing) throw new Error('Missing results');
      const sortFocused = document.activeElement?.matches('[data-sort]');
      syncControls(page);
      existing.replaceWith(region.cloneNode(true));
      for (const [key, value] of Object.entries(incoming.dataset)) directory.dataset[key] = value;
      listUrl = finalUrl;
      directory.dataset.listUrl = finalUrl;
      document.body.dataset.snapshotVersion = incoming.dataset.snapshotVersion;
      syncHead(page);
      const entry: Entry = { kind: 'list', url: finalUrl, scroll: scroll ?? window.scrollY };
      if (mode === 'push' && current() !== finalUrl) { rememberScroll(); history.pushState(entry, '', finalUrl); }
      else if (mode !== 'none' || current() !== finalUrl) history.replaceState(entry, '', finalUrl);
      if (scroll !== undefined) window.scrollTo({ top: scroll, behavior: 'instant' });
      if (sortFocused) directory.querySelector<HTMLElement>('[data-sort]')?.focus({ preventScroll: true });
      return true;
    } catch (error) {
      if (!request.signal.aborted) location.assign(url);
      return false;
    } finally {
      if (request.id === sequence) directory.removeAttribute('aria-busy');
    }
  }
  function formState(): State {
    const params = new URLSearchParams();
    for (const [key, value] of new FormData(form)) if (typeof value === 'string') params.append(key, value);
    const state = readState('/', params.toString());
    state.page = 1;
    return state;
  }
  function change(mode: HistoryMode) { return loadList(stateUrl(formState()), mode); }
  function cancelSearch() { window.clearTimeout(searchTimer); searchSession = false; ++searchEpoch; }
  searchInput().addEventListener('input', event => {
    if ((event as InputEvent).isComposing) return;
    window.clearTimeout(searchTimer);
    // Abort stale results immediately, including during the debounce interval.
    controller?.abort();
    searchTimer = window.setTimeout(() => {
      const mode = searchSession ? 'replace' : 'push';
      const epoch = searchEpoch;
      void change(mode).then(success => { if (success && epoch === searchEpoch) searchSession = true; });
    }, 250);
  });
  searchInput().addEventListener('compositionend', () => searchInput().dispatchEvent(new Event('input')));
  searchInput().addEventListener('blur', () => { searchSession = false; ++searchEpoch; });
  form.addEventListener('change', event => {
    if (!(event.target instanceof HTMLInputElement) || event.target.name === 'q') return;
    cancelSearch(); change('push');
  });
  document.addEventListener('change', event => {
    if ((event.target as Element).matches('[data-sort]')) { cancelSearch(); change('push'); }
  });
  form.addEventListener('submit', event => {
    event.preventDefault(); cancelSearch(); change('push'); filters.close();
  });
  function ordinaryClick(event: MouseEvent, anchor: HTMLAnchorElement) {
    return event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey
      && !anchor.download && (!anchor.target || anchor.target === '_self') && anchor.origin === location.origin;
  }
  document.addEventListener('click', event => {
    const anchor = (event.target as Element).closest<HTMLAnchorElement>('a[href]');
    if (!anchor || !ordinaryClick(event, anchor)) return;
    if (anchor.matches('[data-clear]')) {
      event.preventDefault(); cancelSearch();
      searchInput().value = '';
      form.querySelectorAll<HTMLInputElement>('input[type="checkbox"]').forEach(input => { input.checked = false; });
      form.querySelectorAll<HTMLSelectElement>('[data-sort]').forEach(select => { select.value = 'date-desc'; });
      directory.querySelectorAll<HTMLSelectElement>('[data-sort]').forEach(select => { select.value = 'date-desc'; });
      void loadList('/', 'push');
    } else if (anchor.closest('[data-pagination]')) {
      event.preventDefault(); cancelSearch();
      void loadList(anchor.pathname + anchor.search, 'push', 0);
    } else if (anchor.matches('[data-project-link]') && dialog && modalContent) {
      event.preventDefault(); cancelSearch(); filters.close();
      void openModal(anchor.pathname + anchor.search, 'push', anchor);
    }
  });
  function hideModal(restore = true) {
    if (!dialog?.open) return;
    dialog.close();
    document.documentElement.classList.remove('project-modal-open');
    if (restore && modalContext) {
      window.scrollTo({ top: modalContext.scroll, behavior: 'instant' });
      const opener = modalOpener?.isConnected ? modalOpener : [...directory.querySelectorAll<HTMLAnchorElement>('[data-project-link]')]
        .find(anchor => anchor.pathname === modalContext?.focusHref);
      opener?.focus({ preventScroll: true });
    }
  }
  async function openModal(url: string, mode: HistoryMode, opener?: HTMLElement, context?: Entry) {
    const request = begin();
    directory.removeAttribute('aria-busy');
    const entry: Entry = context || { kind: 'modal', url, listUrl, scroll: window.scrollY,
      focusHref: opener instanceof HTMLAnchorElement ? opener.pathname : undefined };
    try {
      const { page, url: finalUrl } = await fetchPage(url, request.signal);
      if (request.id !== sequence) return;
      const detail = page.querySelector<HTMLElement>('[data-project-detail]');
      if (!detail || detail.dataset.snapshotVersion !== directory.dataset.snapshotVersion) throw new Error('Snapshot changed');
      modalContent!.replaceChildren(detail.cloneNode(true));
      modalContext = { ...entry, url: finalUrl };
      modalOpener = opener || modalOpener;
      if (mode === 'push') { rememberScroll(); history.pushState(modalContext, '', finalUrl); }
      else if (current() !== finalUrl) history.replaceState(modalContext, '', finalUrl);
      if (!dialog!.open) dialog!.showModal();
      dialog!.scrollTop = 0;
      document.documentElement.classList.add('project-modal-open');
      dialog!.querySelector<HTMLElement>('[data-modal-close]')?.focus({ preventScroll: true });
    } catch (error) { if (!request.signal.aborted) location.assign(url); }
  }
  function closeModal() {
    controller?.abort(); ++sequence;
    if ((history.state as Entry | null)?.kind === 'modal') history.back();
    else { hideModal(); history.replaceState({ kind: 'list', url: listUrl, scroll: modalContext?.scroll || 0 }, '', listUrl); }
  }
  dialog?.querySelector('[data-modal-close]')?.addEventListener('click', closeModal);
  dialog?.addEventListener('cancel', event => { event.preventDefault(); closeModal(); });
  dialog?.addEventListener('click', event => { if (event.target === dialog) closeModal(); });
  window.addEventListener('popstate', async event => {
    cancelSearch(); controller?.abort(); ++sequence; filters.close();
    const entry = event.state as Entry | null;
    if (entry?.kind === 'modal') {
      hideModal(false);
      if (entry.listUrl && entry.listUrl !== listUrl) {
        if (!await loadList(entry.listUrl, 'none', entry.scroll)) return;
        // Loading background content must not replace the modal's history URL.
        history.replaceState(entry, '', entry.url);
      }
      await openModal(entry.url, 'none', undefined, entry);
    } else {
      hideModal();
      const url = current();
      if (url !== listUrl) await loadList(url, 'none', entry?.scroll ?? 0);
      else window.scrollTo({ top: entry?.scroll ?? modalContext?.scroll ?? 0, behavior: 'instant' });
    }
  });
}
