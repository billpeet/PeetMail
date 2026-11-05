// The user docs (docs/user/*.md), as Help shows them. Plain TypeScript, so the tests can
// check the real files: every page listed exists, has a title, and every link resolves.

/** Help's pages in sidebar order: docs from docs/user, plus two the app builds itself. */
export const HELP_PAGES = ["getting-started", "shortcuts", "signatures", "sync", "updates", "privacy", "troubleshooting", "report", "whats-new", "your-own-app"] as const;
export type HelpPage = (typeof HELP_PAGES)[number];

/** The pages that are Markdown files in docs/user (the others are made by the app). */
export const DOC_SLUGS = ["getting-started", "signatures", "sync", "updates", "privacy", "troubleshooting", "your-own-app"] as const;

export type Doc = { slug: string; title: string; body: string };

/** A doc's title (its first "# " heading) and the rest of it. */
export function parseDoc(slug: string, md: string): Doc {
  const text = md.replace(/<!--[\s\S]*?-->/g, "");
  const m = /^# +(.+)$/m.exec(text);
  if (!m) return { slug, title: slug, body: text.trim() };
  return { slug, title: m[1].trim(), body: (text.slice(0, m.index) + text.slice(m.index + m[0].length)).trim() };
}

export type LinkTarget = { kind: "page"; page: HelpPage } | { kind: "external"; url: string } | { kind: "none" };

/** Where a link in a doc goes: another Help page, a web page, or nowhere. */
export function resolveLink(href: string): LinkTarget {
  if (/^(https?:|mailto:)/i.test(href)) return { kind: "external", url: href };
  const file = href.split("#")[0].split("/").pop() ?? "";
  if (/^CHANGELOG\.md$/i.test(file)) return { kind: "page", page: "whats-new" };
  const slug = file.replace(/\.md$/i, "");
  return (HELP_PAGES as readonly string[]).includes(slug) && file.endsWith(".md") ? { kind: "page", page: slug as HelpPage } : { kind: "none" };
}

/** Relative links in a doc's Markdown (for the tests' link check). */
export function relativeLinks(md: string): string[] {
  return [...md.matchAll(/\]\(([^)\s]+)\)/g)].map((m) => m[1]).filter((h) => !/^(https?:|mailto:|#)/i.test(h));
}
