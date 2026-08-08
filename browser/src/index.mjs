// The browser signalling binding's public surface.
//
// `T-33` delivers the transport and its platform bindings. The kernel glue over the WebAssembly
// ABI is generated (`A-17`, `docs/specs/browser-sdk.md` §7.2), the media adapter is `M-52`'s and
// the `SipxClient` lifecycle layer is `A-17`'s; none of them are re-exported from here, because a
// convenience this package cannot express over §4 and §5 does not belong in it.

export {
  EventType,
  Reason,
  SIP_SUBPROTOCOL,
  SignallingConfigError,
  SignallingStateError,
  WebSocketSignalling,
  limits,
  signallingUrl,
} from "./transport.mjs";

export { HEADER_OCTETS, RecordFormatError, RecordType, decodeRecord } from "./records.mjs";

export { SignallingCapabilityError, browserPlatform, missingCapability } from "./platform.mjs";
