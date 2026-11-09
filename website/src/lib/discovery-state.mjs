export const FILTERS = {
  category: 'categories',
  audience: 'audience_labels',
  looking: 'looking_for',
};

export function optionsFrom(projects) {
  return Object.fromEntries(Object.entries(FILTERS).map(([name, field]) => [name,
    [...new Set(projects.flatMap((project) => project[field]))].sort((a, b) => a.localeCompare(b)),
  ]));
}

export function readState(url, options) {
  const pathname = new URL(url).pathname;
  const pathPage = Number(pathname.match(/^\/projects\/page\/(\d+)\/$/)?.[1] || 1);
  const params = new URL(url).searchParams;
  const selected = {};
  for (const name of Object.keys(FILTERS)) {
    const allowed = new Set(options[name]);
    selected[name] = [...new Set(params.getAll(name).filter((value) => allowed.has(value)))].sort((a, b) => a.localeCompare(b));
  }
  const rawPage = params.get('page');
  const page = rawPage === null ? pathPage : Number(rawPage);
  return {
    q: (params.get('q') || '').trim(),
    ...selected,
    page: Number.isSafeInteger(page) && page > 0 ? page : 1,
  };
}

export function matchingProjects(projects, state) {
  const query = state.q.toLocaleLowerCase();
  return projects.filter((project) => {
    if (query && !project.searchable_text.toLocaleLowerCase().includes(query)) return false;
    return Object.entries(FILTERS).every(([name, field]) =>
      state[name].length === 0 || state[name].some((value) => project[field].includes(value)));
  });
}

export function pageCount(matches, pageSize) { return Math.max(1, Math.ceil(matches.length / pageSize)); }

export function stateUrl(state) {
  const params = new URLSearchParams();
  if (state.q) params.set('q', state.q);
  for (const name of Object.keys(FILTERS)) for (const value of state[name]) params.append(name, value);
  const hasCriteria = params.size > 0;
  if (state.page > 1 && hasCriteria) params.set('page', String(state.page));
  if (!hasCriteria) return state.page > 1 ? `/projects/page/${state.page}/` : '/projects/';
  return `/projects/?${params}`;
}
