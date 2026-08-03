#!/usr/bin/env node
// Prove that the installed site's broken-anchor checker rejects a missing id.
//
// This deliberately calls the same handler used at the end of a site build, with the real site's
// configured severities. Building the entire site a second time made this assertion depend on a
// second compiler and worker lifecycle; under full-workspace load that process occasionally exited
// through Node's unsettled-top-level-await path before the checker answered the question.

import {createRequire} from 'node:module';

const require = createRequire(new URL('../website/package.json', import.meta.url));
const config = require('./docusaurus.config.js');
const {handleBrokenLinks} = require('@docusaurus/core/lib/server/brokenLinks');

const docs = `${config.baseUrl.replace(/\/$/, '')}/docs`;
const missing = 'zz-no-page-emits-this-id';
const probe = `${docs}/zz-anchor-guard-probe`;
const collectedLinks = {
  [docs]: {links: [], anchors: []},
  [`${docs}/`]: {links: [], anchors: []},
  [probe]: {links: [`${docs}#${missing}`], anchors: []},
};

try {
  await handleBrokenLinks({
    collectedLinks,
    routes: [],
    onBrokenLinks: config.onBrokenLinks,
    onBrokenAnchors: config.onBrokenAnchors,
  });
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);
  if (/broken anchors/i.test(message) && message.includes(missing)) {
    console.log('anchors: a link to an id no page emits fails the build');
    process.exit(0);
  }
  throw error;
}

console.error('a link to an id no page emits BUILT SUCCESSFULLY.');
console.error('the site\'s broken-anchor guard is not armed: check onBrokenAnchors.');
process.exit(1);
