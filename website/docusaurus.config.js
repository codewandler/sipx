// The public documentation site. This is the customer-facing view of sipx; the internal
// contributor material (stories, designs, specs, roadmap) stays in ../docs and is deliberately
// not published here — see ../docs/README.md for the split.
//
// Built on every push and pull request (a dead link fails the build: the reporting handlers
// below); deployed from main by CI, with the rustdoc API reference copied in under /api.

// @ts-check

/** @type {import('@docusaurus/types').Config} */
const config = {
  title: 'sipx',
  tagline:
    'A SIP and VoIP stack in Rust. Place and answer calls, register against a PBX, carry real audio.',
  url: 'https://codewandler.github.io',
  baseUrl: '/sipx/',
  organizationName: 'codewandler',
  projectName: 'sipx',
  deploymentBranch: 'gh-pages',
  trailingSlash: false,

  // -- what a defect in the site does to the build (X-41) -------------------------------------
  //
  // Docusaurus reports these four independently, and three of the four default to `warn`, which
  // prints the defect and exits 0. The `docs site` gate step is a step whose exit code is the
  // only thing anyone reads, so a `warn` here is a green gate with a dead link in the published
  // site — which is exactly what happened: `onBrokenAnchors` was unset, `S-30` linked to
  // `#cli-reference` (an id Docusaurus does not emit for a page's `h1`, because the first `h1`
  // becomes the page header), the step printed the broken anchor and the gate reported all
  // steps green. Every one of the four is therefore stated here rather than inherited, with the
  // reason, so that "it defaults to something sensible" is never the answer again.
  //
  // All four are `throw`. The site is what the README points at as a measurement and the
  // compliance table is served from it, so there is no defect in this list a reader should meet
  // before we do. If one of them ever has to be relaxed, it belongs here with a reason next to
  // it and a story against it — not as a deleted line.

  // A link to a page the site does not publish. Docusaurus already defaults this to `throw`;
  // stated anyway, because the default is the only one of the four that is strict and a reader
  // cannot tell a deliberate `throw` from an absent line.
  onBrokenLinks: 'throw',

  // A link to an `#anchor` no page on the site emits. Defaults to `warn` (Docusaurus plans
  // `throw` for v4). This is the defect X-41 exists for.
  onBrokenAnchors: 'throw',

  // Two routes claiming the same path — one page silently wins and the other becomes
  // unreachable. Defaults to `warn`. Not a link defect, but the same failure: printed, and
  // exited 0. A page nobody can reach is worse than a page that 404s, because the sidebar
  // still offers it.
  onDuplicateRoutes: 'throw',

  favicon: 'img/logo.svg',

  markdown: {
    mermaid: true,
    hooks: {
      // A relative Markdown link (`./guides/register.md`) that resolves to no file. Defaults to
      // `warn`, and it is a *separate* default from `onBrokenLinks` above: the resolution
      // happens while the Markdown is compiled, before any route exists, so `onBrokenLinks`
      // never sees it. Was already `throw`; kept, and now audited alongside the other three
      // rather than being the one setting somebody remembered.
      onBrokenMarkdownLinks: 'throw',
    },
  },

  presets: [
    [
      'classic',
      /** @type {import('@docusaurus/preset-classic').Options} */
      ({
        docs: {
          path: 'docs',
          routeBasePath: 'docs',
          sidebarPath: require.resolve('./sidebars.js'),
          editUrl: 'https://github.com/codewandler/sipx/tree/main/website/',
        },
        blog: false,
        theme: {
          customCss: require.resolve('./src/css/custom.css'),
        },
      }),
    ],
  ],

  themes: [
    '@docusaurus/theme-mermaid',
    [
      // Offline, index-based search — no external service needed.
      require.resolve('@easyops-cn/docusaurus-search-local'),
      {
        hashed: true,
        indexBlog: false,
        docsRouteBasePath: 'docs',
      },
    ],
  ],

  plugins: [
    // Emits /llms.txt (curated index from the sidebar) and /llms-full.txt (the whole corpus)
    // at build time, so neither can drift from the published pages.
    require.resolve('./plugins/llms-txt'),
  ],

  themeConfig:
    /** @type {import('@docusaurus/preset-classic').ThemeConfig} */
    ({
      navbar: {
        title: 'sipx',
        logo: { alt: '', src: 'img/logo.svg' },
        items: [
          { type: 'docSidebar', sidebarId: 'docs', position: 'left', label: 'Docs' },
          { to: '/docs/getting-started', label: 'CLI', position: 'left' },
          { to: '/docs/guides/as-a-library', label: 'Rust libraries', position: 'left' },
          {
            href: 'https://codewandler.github.io/sipx/api/',
            label: 'API',
            position: 'right',
          },
          { href: 'https://github.com/codewandler/sipx', label: 'GitHub', position: 'right' },
        ],
      },
      footer: {
        style: 'dark',
        links: [
          {
            title: 'Docs',
            items: [
              { label: 'Getting started', to: '/docs/getting-started' },
              { label: 'Does sipx fit?', to: '/docs/guides/does-this-fit' },
              {
                label: 'Integrate an endpoint',
                to: '/docs/guides/integrate-existing-system',
              },
              { label: 'Troubleshooting', to: '/docs/guides/troubleshooting' },
            ],
          },
          {
            title: 'Libraries',
            items: [
              { label: 'Choose a crate', to: '/docs/guides/as-a-library' },
              { label: 'Place a call', to: '/docs/guides/place-a-call' },
              { label: 'Answer a call', to: '/docs/guides/answer-a-call' },
              { label: 'Register', to: '/docs/guides/register' },
            ],
          },
          {
            title: 'Project',
            items: [
              { label: 'GitHub', href: 'https://github.com/codewandler/sipx' },
              { label: 'API reference', href: 'https://codewandler.github.io/sipx/api/' },
              { label: 'Security', to: '/docs/reference/security' },
              { label: 'RFC compliance', to: '/docs/reference/compliance' },
              { label: 'How sipx is built', to: '/docs/reference/development-process' },
              { label: "What's new", to: '/docs/whats-new' },
              { label: 'Application host', to: '/docs/sdk/overview' },
            ],
          },
        ],
        copyright: `Copyright © ${new Date().getFullYear()} the sipx contributors. MIT OR Apache-2.0.`,
      },
      prism: {
        additionalLanguages: ['bash', 'json', 'toml', 'rust'],
      },
    }),
};

module.exports = config;
