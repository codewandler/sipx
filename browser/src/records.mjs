// The §4.6 output-record framing, decoded.
//
// `docs/specs/browser-sdk.md` §4.6 defines one record as a little-endian `u32` type, a
// little-endian `u32` payload length, and the payload. This module turns those octets into the
// tagged values the binding switches on, and does nothing else: no state, no I/O, no kernel.

/**
 * The §4.6 record types.
 *
 * **Not exhaustive by design.** §5.1 makes appended record types a compatible change within
 * `sipx.browser.v1`, so a reader that meets an unknown type has met a newer kernel, not a corrupt
 * one. {@link decodeRecord} still refuses it — an unrecognised record cannot be acted on, and
 * guessing is worse than refusing — but the refusal is the caller's to classify.
 */
export const RecordType = Object.freeze({
  /** Raw bytes of exactly one SIP message, to be sent as one WebSocket message. */
  Wire: 1,
  /** A `u64` timer id and a `u64` absolute `fire_at_ms`. */
  TimerSet: 2,
  /** A `u64` timer id the host should clear. */
  TimerCancel: 3,
  /** One §5.3 JSON event document. */
  Event: 4,
});

/** Octets of framing in front of every record's payload. */
export const HEADER_OCTETS = 8;

const decoder = new TextDecoder("utf-8", { fatal: true });

/** Thrown when a buffer is not a well-formed §4.6 record. A kernel defect, never peer input. */
export class RecordFormatError extends Error {}

/**
 * Decode one §4.6 record.
 *
 * Returns one of:
 *
 * - `{ kind: "wire", bytes: Uint8Array }`
 * - `{ kind: "timer-set", id: bigint, fireAtMs: number }`
 * - `{ kind: "timer-cancel", id: bigint }`
 * - `{ kind: "event", document: string }`
 *
 * Timer **ids** stay `bigint` because they are identity: they are compared and used as map keys,
 * never counted, and the ABI makes them `u64` with no reuse. `fireAtMs` becomes a `number`
 * because it is arithmetic — the binding subtracts `now` from it — and a monotonic millisecond
 * clock will not reach `Number.MAX_SAFE_INTEGER` in any page's lifetime. A value that somehow did
 * is refused rather than silently rounded.
 *
 * Everything returned is **copied out** of the input. §4.4 makes a kernel-owned buffer valid only
 * until the next call of any export on that handle, so a decoder that returned a view into it
 * would hand the caller a reference that expires on the next line.
 *
 * @param {Uint8Array} framed one complete record, header included
 * @throws {RecordFormatError} on a truncated, over-long or unrecognised record
 */
export function decodeRecord(framed) {
  if (framed.length < HEADER_OCTETS) {
    throw new RecordFormatError("an output record is shorter than its own header");
  }
  const view = new DataView(framed.buffer, framed.byteOffset, framed.byteLength);
  const type = view.getUint32(0, true);
  const length = view.getUint32(4, true);
  if (HEADER_OCTETS + length > framed.length) {
    throw new RecordFormatError("an output record's payload runs past the buffer");
  }
  const payload = framed.subarray(HEADER_OCTETS, HEADER_OCTETS + length);

  switch (type) {
    case RecordType.Wire:
      return { kind: "wire", bytes: payload.slice() };
    case RecordType.TimerSet: {
      if (length !== 16) throw new RecordFormatError("a TIMER_SET payload is not 16 octets");
      const timers = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
      const fireAt = timers.getBigUint64(8, true);
      if (fireAt > BigInt(Number.MAX_SAFE_INTEGER)) {
        throw new RecordFormatError("a TIMER_SET deadline exceeds the exactly-representable range");
      }
      return { kind: "timer-set", id: timers.getBigUint64(0, true), fireAtMs: Number(fireAt) };
    }
    case RecordType.TimerCancel: {
      if (length !== 8) throw new RecordFormatError("a TIMER_CANCEL payload is not 8 octets");
      const timers = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
      return { kind: "timer-cancel", id: timers.getBigUint64(0, true) };
    }
    case RecordType.Event:
      try {
        return { kind: "event", document: decoder.decode(payload) };
      } catch {
        throw new RecordFormatError("an EVENT payload is not UTF-8");
      }
    default:
      throw new RecordFormatError(`unrecognised output record type ${type}`);
  }
}
