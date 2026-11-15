export function mobileFilters() {
  const sidebar = document.querySelector<HTMLElement>('.site-sidebar');
  const toggle = document.querySelector<HTMLButtonElement>('[data-filters-toggle]');
  if (!sidebar || !toggle) return { close() {} };
  const media = matchMedia('(max-width: 767px)');
  let scroll = 0;
  let opener: HTMLElement | null = null;
  const background = () => [...document.querySelectorAll<HTMLElement>('body > main, body > header, body > footer')];
  function close(restoreFocus = true) {
    if (!sidebar!.hasAttribute('data-mobile-filters-open')) return;
    sidebar!.removeAttribute('data-mobile-filters-open');
    sidebar!.removeAttribute('role');
    sidebar!.removeAttribute('aria-modal');
    document.documentElement.classList.remove('mobile-filters-open');
    document.body.classList.remove('mobile-filters-open');
    toggle!.setAttribute('aria-expanded', 'false');
    background().forEach(node => { node.inert = false; });
    window.scrollTo({ top: scroll, behavior: 'instant' });
    if (restoreFocus) (media.matches ? opener : sidebar!.querySelector<HTMLElement>('input'))?.focus({ preventScroll: true });
  }
  function open() {
    if (!media.matches) return;
    scroll = window.scrollY;
    opener = document.activeElement as HTMLElement;
    sidebar!.setAttribute('data-mobile-filters-open', '');
    sidebar!.setAttribute('role', 'dialog');
    sidebar!.setAttribute('aria-modal', 'true');
    document.documentElement.classList.add('mobile-filters-open');
    document.body.classList.add('mobile-filters-open');
    toggle!.setAttribute('aria-expanded', 'true');
    background().forEach(node => { node.inert = true; });
    sidebar!.querySelector<HTMLElement>('input')?.focus({ preventScroll: true });
  }
  toggle.addEventListener('click', () => sidebar.hasAttribute('data-mobile-filters-open') ? close() : open());
  sidebar.addEventListener('click', event => { if ((event.target as Element).closest('[data-filters-close]')) close(); });
  sidebar.addEventListener('keydown', event => {
    if (!sidebar.hasAttribute('data-mobile-filters-open')) return;
    if (event.key === 'Escape') { event.preventDefault(); close(); }
    if (event.key !== 'Tab') return;
    const items = [...sidebar.querySelectorAll<HTMLElement>('a[href], button, input, select, [tabindex="0"]')]
      .filter(node => node.getClientRects().length && !(node as HTMLButtonElement).disabled);
    const first = items[0], last = items.at(-1);
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
  });
  media.addEventListener('change', () => { if (!media.matches) close(); });
  return { close: () => close(false) };
}
