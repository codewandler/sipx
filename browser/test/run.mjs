// Run the browser signalling binding's cases.
//
//   node browser/test/run.mjs [--module <path to sipx_browser.wasm>]
//
// Without `--module` only the cases that drive the binding against fakes run. With it, the
// binding is additionally driven against the real compiled kernel — the same artifact
// `wasm/harness.mjs` checks — which is what turns "feeds received bytes to the kernel and sends
// only bytes emitted by it" from a statement about a fake into a statement about the shipped
// module. `scripts/check-browser-binding.sh` always passes it.

import { argv, exit } from "node:process";

import { run } from "./assert.mjs";

await import("./transport.test.mjs");

const flag = argv.indexOf("--module");
const modulePath = flag >= 0 ? argv[flag + 1] : undefined;
if (modulePath) {
  const { register } = await import("./kernel.test.mjs");
  await register(modulePath);
} else {
  console.log("note: --module was not given, so the compiled-kernel cases did not run");
}

const failures = await run("browser signalling binding");
if (failures > 0) {
  console.error(`\n${failures} case(s) failed`);
  exit(1);
}
console.log("\nbrowser signalling binding: every case passed");
