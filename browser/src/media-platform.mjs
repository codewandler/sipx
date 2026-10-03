// Browser globals for the native-media adapter, separate from the signalling platform.
export class MediaCapabilityError extends Error {
  constructor(capability) {
    super(`this browser does not provide ${capability}`);
    this.name = "MediaCapabilityError";
    this.capability = capability;
  }
}
export function browserMediaPlatform(scope = globalThis) {
  for (const [name, present] of [
    ["secure context", () => scope.isSecureContext === true],
    ["RTCPeerConnection", () => typeof scope.RTCPeerConnection === "function"],
    [
      "media capture",
      () => typeof scope.navigator?.mediaDevices?.getUserMedia === "function",
    ],
    [
      "audio playback",
      () => typeof scope.document?.createElement === "function",
    ],
  ])
    if (!present()) throw new MediaCapabilityError(name);
  return {
    clock: {
      setTimer: (fn, ms) => scope.setTimeout(fn, ms),
      clearTimer: (id) => scope.clearTimeout(id),
    },
    getUserMedia: (constraints) =>
      scope.navigator.mediaDevices.getUserMedia(constraints),
    createPeerConnection: (options) => new scope.RTCPeerConnection(options),
    codecs: () => scope.RTCRtpSender?.getCapabilities?.("audio")?.codecs ?? [],
    createPlayback(event) {
      const audio = scope.document.createElement("audio");
      audio.autoplay = true;
      audio.srcObject =
        event.streams?.[0] ?? new scope.MediaStream([event.track]);
      const close = () => {
        audio.pause();
        audio.srcObject = null;
        audio.remove();
      };
      try {
        return { ready: audio.play(), close };
      } catch (error) {
        close();
        throw error;
      }
    },
    onPageHide: (handler) => {
      scope.addEventListener("pagehide", handler);
      return () => scope.removeEventListener("pagehide", handler);
    },
  };
}
