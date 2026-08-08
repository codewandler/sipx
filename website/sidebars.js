// Curated on purpose: this is what a reader who does not have the repository open should see,
// in the order they should see it. Internal material (stories, designs, specs, roadmap) is not
// published here at all — see ../docs/README.md.

/** @type {import('@docusaurus/plugin-content-docs').SidebarsConfig} */
const sidebars = {
  docs: [
    {
      type: 'category',
      label: 'Start',
      collapsed: false,
      items: [
        'intro',
        'getting-started',
        'architecture',
        'guides/does-this-fit',
        'guides/integrate-existing-system',
      ],
    },
    {
      type: 'category',
      label: 'CLI',
      collapsed: false,
      items: ['reference/cli', 'guides/troubleshooting'],
    },
    {
      type: 'category',
      label: 'Rust libraries',
      collapsed: false,
      items: [
        'guides/as-a-library',
        'guides/test-a-call',
        'guides/rtp-echo',
        'guides/place-a-call',
        'guides/answer-a-call',
        'guides/hold-and-resume',
        'guides/send-and-collect-dtmf',
        'guides/play-audio',
        'guides/record-a-call',
        'guides/blind-transfer',
        'guides/attended-transfer',
        'guides/couple-two-calls',
        'guides/persist-a-dialog',
        'guides/register',
        'guides/export-observability',
      ],
    },
    {
      type: 'category',
      label: 'Reference',
      items: [
        'reference/security',
        'reference/privacy',
        'reference/logging',
        'reference/compliance',
        'reference/comparison',
        'reference/development-process',
        'reference/diagnostic-phone-proof',
        'reference/browser-audio-proof',
        'whats-new',
      ],
    },
    {
      type: 'category',
      label: 'Application host',
      items: ['sdk/overview', 'sdk/contract'],
    },
  ],
};

module.exports = sidebars;
