export function sharedDate(seconds) {
  return new Intl.DateTimeFormat('en-US', { year: 'numeric', month: 'short', day: 'numeric', timeZone: 'UTC' }).format(new Date(seconds * 1000));
}
export function projectTitle(project) { return project.name?.trim() || 'Unnamed project'; }
export function categoryLabel(slug, categories) { return categories.find((item) => item.slug === slug)?.label || slug; }
export function safeWebUrl(value) {
  try { const url = new URL(value); return ['https:', 'http:'].includes(url.protocol) ? url.href : null; }
  catch { return null; }
}
