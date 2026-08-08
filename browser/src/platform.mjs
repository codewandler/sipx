// The only file in this package that reads a global.
//
// `transport.mjs` takes its socket, clock, entropy source and connectivity monitor as arguments,
// which is what makes it testable without a browser and substitutable in a diagnostic build.
// This module supplies the real ones, and is deliberately the whole of the platform surface: if a
// browser API is used by this package and is not named here, that is a defect.

/**
 * The capabilities §7.3 requires of the host, restricted to the ones *this* binding needs.
 *
 * `RTCPeerConnection`, media capture and the rest of §7.3's table belong to the media adapter and
 * the lifecycle layer; a signalling transport that refused to load because a page had no
 * microphone would be refusing on someone else's behalf. The full check is `SipxClient.create`'s.
 *
 * Ordered, and reported one at a time, so the failure names the first thing that is missing
 * rather than a list a reader has to intersect with their own browser.
 */
const REQUIRED = Object.freeze([
  ["WebSocket", (scope) => typeof scope.WebSocket === "function"],
  ["secure context", (scope) => scope.isSecureContext !== false],
  ["crypto.getRandomValues", (scope) => typeof scope.crypto?.getRandomValues === "function"],
  ["performance.now", (scope) => typeof scope.performance?.now === "function"],
  ["queueMicrotask", (scope) => typeof scope.queueMicrotask === "function"],
]);

/** A host that cannot carry this binding. Names the first missing interface (§6.6). */
export class SignallingCapabilityError extends Error {
  constructor(capability) {
    super(`this browser does not provide ${capability}`);
    this.name = "SignallingCapabilityError";
    /** @type {string} the interface that was missing */
    this.capability = capability;
  }
}

/**
 * The first capability the host does not provide, or `undefined`.
 *
 * @param {typeof globalThis} [scope]
 */
export function missingCapability(scope = globalThis) {
  for (const [name, present] of REQUIRED) {
    let ok = false;
    try {
      ok = present(scope);
    } catch {
      ok = false;
    }
    if (!ok) return name;
  }
  return undefined;
}

/**
 * The browser's facilities, in the shape {@link import("./transport.mjs").WebSocketSignalling}
 * expects.
 *
 * Fails closed: a host missing any required capability throws before a socket is constructed,
 * because §6.2 does not allow a half-started client and a transport that discovered halfway
 * through a registration that the page has no CSPRNG would have no honest way to finish.
 *
 * Two choices worth stating. `performance.now` rather than `Date.now`, because §4.2 makes
 * `now_ms` monotonic and wall-clock time never crosses the ABI — a clock that can step backwards
 * would earn `E_TIME` on the next daylight-saving change. And `queueMicrotask` rather than a
 * timer, because §6.4 rule 2 asks only that a listener not run *inside* the handler that drove
 * the kernel, and a microtask settles in the same turn rather than adding a scheduling delay to
 * every received message.
 *
 * @param {typeof globalThis} [scope]
 * @throws {SignallingCapabilityError}
 */
export function browserPlatform(scope = globalThis) {
  const missing = missingCapability(scope);
  if (missing !== undefined) throw new SignallingCapabilityError(missing);

  return {
    openSocket: (url, protocols) => new scope.WebSocket(url, protocols),
    clock: {
      now: () => scope.performance.now(),
      setTimer: (callback, delayMs) => scope.setTimeout(callback, delayMs),
      clearTimer: (handle) => scope.clearTimeout(handle),
      defer: (task) => scope.queueMicrotask(task),
    },
    entropy: {
      // §4.7 and §8.4: this, and nothing else. There is no fallback path here to find, which is
      // the point — `Math.random` appears in no file of this package.
      fill: (length) => scope.crypto.getRandomValues(new Uint8Array(length)),
    },
    connectivity: {
      // `navigator.onLine` is a hint and a false negative is impossible to distinguish from a
      // network that simply went away, so the binding treats an absent `navigator` as online and
      // lets the socket report the truth.
      isOnline: () => scope.navigator?.onLine !== false,
      subscribe: (handler) => {
        const online = () => handler(true);
        const offline = () => handler(false);
        scope.addEventListener("online", online);
        scope.addEventListener("offline", offline);
        return () => {
          scope.removeEventListener("online", online);
          scope.removeEventListener("offline", offline);
        };
      },
    },
  };
}
