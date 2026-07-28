// Curated on purpose: this is what a reader who does not have the repository open should see,
// in the order they should see it. Internal material (stories, designs, specs, roadmap) is not
// published here at all — see ../docs/README.md.

/** @type {import('@docusaurus/plugin-content-docs').SidebarsConfig} */
const sidebars = {
  docs: [
    'intro',
    'getting-started',
    {
      type: 'category',
      label: 'Guides',
      collapsed: false,
      items: [
        'guides/does-this-fit',
        'guides/place-a-call',
        'guides/answer-a-call',
        'guides/register',
        'guides/as-a-library',
      ],
    },
    {
      type: 'category',
      label: 'SDK (preview)',
      items: ['sdk/overview', 'sdk/contract'],
    },
    {
      type: 'category',
      label: 'Migrate to sipx',
      collapsed: false,
      items: ['migrate/from-kamailio', 'migrate/from-asterisk'],
    },
    {
      type: 'category',
      label: 'Reference',
      items: ['reference/cli', 'reference/compliance'],
    },
    'whats-new',
  ],
};

module.exports = sidebars;
