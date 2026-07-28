// The public documentation site. This is the customer-facing view of sipx; the internal
// contributor material (stories, designs, specs, roadmap) stays in ../docs and is deliberately
// not published here — see ../docs/README.md for the split.
//
// Built on every push and pull request (a broken link fails the build: onBrokenLinks below);
// deployed from main by CI, with the rustdoc API reference copied in under /api.

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

  onBrokenLinks: 'throw',

  favicon: 'img/logo.svg',

  markdown: {
    mermaid: true,
    hooks: {
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
          { to: '/docs/sdk/overview', label: 'SDK', position: 'left' },
          { to: '/docs/whats-new', label: "What's new", position: 'left' },
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
              { label: 'SDK', to: '/docs/sdk/overview' },
              { label: 'Migrate', to: '/docs/migrate/from-kamailio' },
            ],
          },
          {
            title: 'Project',
            items: [
              { label: 'GitHub', href: 'https://github.com/codewandler/sipx' },
              { label: 'API reference', href: 'https://codewandler.github.io/sipx/api/' },
              {
                label: 'RFC compliance',
                href: 'https://github.com/codewandler/sipx/blob/main/docs/compliance.md',
              },
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
