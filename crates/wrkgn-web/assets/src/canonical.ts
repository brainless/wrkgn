/** These rules mirror discovery.rs; both suites consume canonical-urls.json. */
export const SORTS = ['comments-asc', 'comments-desc', 'date-desc', 'date-asc'] as const;
export type Sort = typeof SORTS[number];
export type Facet = 'category' | 'audience';
/** A criterion fixed by the path (landing page); not a query criterion. */
export type Fixed = { facet: Facet; slug: string };
export type State = { q: string; category: string[]; audience: string[]; sort: Sort; page: number; fixed: Fixed | null };
/** Facets with landing pages; add 'audience' (and its path) to enable audiences. */
export const LANDING: Partial<Record<Facet, string>> = { category: '/categories/', audience: '/audiences/' };
export function parseLanding(path: string): { fixed: Fixed; page: string | null } | null {
  for (const [facet, prefix] of Object.entries(LANDING) as [Facet, string][]) {
    if (!path.startsWith(prefix)) continue;
    const match = path.slice(prefix.length).match(/^([^/]+)\/(?:page\/([^/]+)\/)?$/);
    if (match) return { fixed: { facet, slug: match[1] }, page: match[2] ?? null };
  }
  return null;
}
const landingPath = (fixed: Fixed) => `${LANDING[fixed.facet]}${fixed.slug}/`;
const digitsPage = (segment: string | null) => segment && /^\d+$/.test(segment) ? validPage(segment) : 1;
export type Options = { category: string[]; audience: string[] };
const compare = (a: string, b: string) => {
  const aa = Array.from(a, c => c.codePointAt(0)!);
  const bb = Array.from(b, c => c.codePointAt(0)!);
  for (let i = 0; i < Math.min(aa.length, bb.length); i++) if (aa[i] !== bb[i]) return aa[i] - bb[i];
  return aa.length - bb.length;
};
const values = (items: string[]) => [...new Set(items)].sort(compare);
export const normalizeQuery = (q: string) => Array.from(q.trim()).slice(0, 100).join('').trimEnd();
const validPage = (raw: string | number) => {
  const page = Number(raw);
  return Number.isSafeInteger(page) && page > 0 ? page : 1;
};
export function readState(path: string, query = ''): State {
  const params = new URLSearchParams(query);
  // Landing pages ignore query filters, search and sort; only `page` is read.
  const landing = parseLanding(path);
  if (landing) {
    const page = params.has('page') ? validPage(params.get('page')!) : digitsPage(landing.page);
    return { q: '', category: [], audience: [], sort: 'date-desc', page, fixed: landing.fixed };
  }
  const pathPage = path.match(/^\/projects\/page\/(\d+)\/$/);
  const sort = params.get('sort') as Sort;
  return {
    q: normalizeQuery(params.get('q') || ''),
    category: values(params.getAll('category')),
    audience: values(params.getAll('audience')),
    sort: SORTS.includes(sort) ? sort : 'date-desc',
    page: validPage(params.get('page') ?? pathPage?.[1] ?? 1),
    fixed: null,
  };
}
export function stateUrl(state: State): string {
  if (state.fixed) {
    const page = validPage(state.page);
    return page > 1 ? `${landingPath(state.fixed)}page/${page}/` : landingPath(state.fixed);
  }
  const params = new URLSearchParams();
  const q = normalizeQuery(state.q);
  if (q) params.append('q', q);
  for (const name of ['category', 'audience'] as const) for (const value of values(state[name])) params.append(name, value);
  const sort = SORTS.includes(state.sort) ? state.sort : 'date-desc';
  if (sort !== 'date-desc') params.append('sort', sort);
  const page = validPage(state.page);
  if (!params.size) return page > 1 ? `/projects/page/${page}/` : '/';
  if (page > 1) params.append('page', String(page));
  return `/?${params}`;
}
/** Snapshot-dependent page counts stay server-owned; tests inject the count. */
export function resolveUrl(input: string, options: Options, count: (state: State) => number) {
  const [path, ...query] = input.split('?');
  const state = readState(path, query.join('?'));
  const syntax = stateUrl(state);
  if (syntax !== input) return { status: 301, location: syntax };
  if (state.fixed && !options[state.fixed.facet].includes(state.fixed.slug)) return { status: 404, location: null };
  for (const name of ['category', 'audience'] as const) state[name] = state[name].filter(value => options[name].includes(value));
  state.page = Math.min(state.page, Math.max(1, Math.ceil(count(state) / 30)));
  const canonical = stateUrl(state);
  return canonical === input ? { status: null, location: null } : { status: 302, location: canonical };
}
