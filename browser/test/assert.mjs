// The whole test framework, because the alternative is a dependency.
//
// `wasm/harness.mjs` established the shape this file generalises: hand-rolled `check`/`equal`,
// a failure counter, and a non-zero exit. That harness asserts one artifact and reads top to
// bottom; a binding with a dozen independent cases needs the cases named and isolated, so this
// adds a registry and a per-case try/catch and stops there. No runner, no reporter, no plugins:
// the repository has no JavaScript toolchain and this story is not the place to introduce one.

/** @type {{name: string, body: () => void | Promise<void>}[]} */
const registered = [];

/** Register one case. Bodies may be async; they run in registration order, one at a time. */
export function test(name, body) {
  registered.push({ name, body });
}

/** Thrown by the assertions below. Carries no stack trimming — the raw one is more useful. */
class AssertionFailed extends Error {}

/** Assert a condition, naming the property rather than the expression. */
export function check(condition, description) {
  if (!condition) throw new AssertionFailed(description);
}

/** Assert strict equality, reporting both sides. */
export function equal(actual, expected, description) {
  if (actual !== expected) {
    throw new AssertionFailed(
      `${description}\n         expected ${JSON.stringify(expected)}\n         actual   ${JSON.stringify(actual)}`,
    );
  }
}

/** Assert deep equality over JSON-shaped values, reporting both sides. */
export function deepEqual(actual, expected, description) {
  const left = JSON.stringify(actual);
  const right = JSON.stringify(expected);
  if (left !== right) {
    throw new AssertionFailed(
      `${description}\n         expected ${right}\n         actual   ${left}`,
    );
  }
}

/** Assert that `body` throws, and return what it threw so the caller can inspect it. */
export function throws(body, description) {
  try {
    body();
  } catch (error) {
    return error;
  }
  throw new AssertionFailed(`${description} (nothing was thrown)`);
}

/**
 * Run every registered case.
 *
 * Returns the number of failures so the caller owns the exit code; a case that throws anything
 * at all fails rather than aborting the run, because one broken case must not hide the rest.
 */
export async function run(label) {
  console.log(`${label} — ${registered.length} cases`);
  let failures = 0;
  for (const { name, body } of registered) {
    try {
      await body();
      console.log(`  ok   ${name}`);
    } catch (error) {
      failures += 1;
      const detail = error instanceof AssertionFailed ? error.message : `${error?.stack ?? error}`;
      console.log(`  FAIL ${name}\n         ${detail}`);
    }
  }
  return failures;
}
