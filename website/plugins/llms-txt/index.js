// Build-time generation of /llms.txt and /llms-full.txt from the docs sidebar.
//
// llms.txt is a curated index: one link list per sidebar category, each entry with a one-line
// blurb taken from the page's frontmatter description (or its first prose paragraph).
// llms-full.txt inlines the whole corpus in sidebar order. Both are derived from the same
// sidebar the site navigation uses, so they can never drift from the published pages.

const fs = require('fs');
const path = require('path');

const SITE_URL = 'https://codewandler.github.io/sipx';

function readDoc(docsDir, id) {
  const file = path.join(docsDir, `${id}.md`);
  if (!fs.existsSync(file)) return null;
  const raw = fs.readFileSync(file, 'utf8');
  let title = null;
  let description = null;
  let slug = null;
  let body = raw;
  const fm = raw.match(/^---\n([\s\S]*?)\n---\n/);
  if (fm) {
    body = raw.slice(fm[0].length);
    const t = fm[1].match(/^title:\s*(["']?)(.+?)\1\s*$/m);
    if (t) title = t[2];
    const d = fm[1].match(/^description:\s*(["']?)(.+?)\1\s*$/m);
    if (d) description = d[2];
    const s = fm[1].match(/^slug:\s*(["']?)(.+?)\1\s*$/m);
    if (s) slug = s[2];
  }
  if (!title) {
    const h1 = body.match(/^#\s+(.+)$/m);
    if (h1) title = h1[1];
  }
  if (!description) {
    // First prose paragraph: skip headings, fences, comments, admonitions, blank lines.
    const lines = body.split('\n');
    let inFence = false;
    for (const line of lines) {
      const l = line.trim();
      if (l.startsWith('```')) { inFence = !inFence; continue; }
      if (inFence || l === '' || l.startsWith('#') || l.startsWith('<!--') ||
          l.startsWith(':::') || l.startsWith('|') || l.startsWith('import ')) continue;
      description = l.replace(/\s+/g, ' ');
      break;
    }
  }
  const route = slug === '/' ? '' : `/${(slug ?? id).replace(/^\//, '')}`;
  return { id, route, title: title ?? id, description: description ?? '', body };
}

function flatten(items, category) {
  const out = [];
  for (const item of items) {
    if (typeof item === 'string') {
      out.push({ id: item, category });
    } else if (item && item.type === 'category') {
      out.push(...flatten(item.items, item.label));
    } else if (item && item.type === 'ref' && item.id) {
      // Cross-listed elsewhere; skip to avoid duplicates.
    }
  }
  return out;
}

module.exports = function llmsTxtPlugin(context) {
  return {
    name: 'llms-txt',
    async postBuild({ outDir }) {
      const siteDir = context.siteDir;
      const docsDir = path.join(siteDir, 'docs');
      // eslint-disable-next-line import/no-dynamic-require
      const sidebars = require(path.join(siteDir, 'sidebars.js'));
      const entries = flatten(sidebars.docs, 'Start here')
        .map(({ id, category }) => ({ category, doc: readDoc(docsDir, id) }))
        .filter((e) => e.doc !== null);

      const index = ['# sipx', '', '> ' + context.siteConfig.tagline, ''];
      let currentCategory = null;
      for (const { category, doc } of entries) {
        if (category !== currentCategory) {
          index.push(`## ${category}`, '');
          currentCategory = category;
        }
        const url = `${SITE_URL}/docs${doc.route}`;
        index.push(`- [${doc.title}](${url})${doc.description ? ': ' + doc.description : ''}`);
      }
      index.push('');

      const full = [];
      for (const { doc } of entries) {
        full.push(`# ${doc.title}`, '', doc.body.trim(), '', '---', '');
      }

      fs.writeFileSync(path.join(outDir, 'llms.txt'), index.join('\n'));
      fs.writeFileSync(path.join(outDir, 'llms-full.txt'), full.join('\n'));
    },
  };
};
