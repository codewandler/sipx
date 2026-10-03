import { contract } from "../generated/abi.mjs";
export { SipxMediaError } from "./media.mjs";
export class SipxAbiDefect extends Error {
  constructor(code) {
    super("browser kernel contract failed");
    this.name = "SipxAbiDefect";
    this.code = code;
  }
}
export class SipxStateError extends Error {
  constructor() {
    super("operation is not allowed in this state");
    this.name = "SipxStateError";
  }
}
export class SipxLimitError extends Error {
  constructor(kind = "limit") {
    super("browser resource limit exceeded");
    this.name = "SipxLimitError";
    this.kind = kind;
  }
}
export class SipxTransportError extends Error {
  constructor(reason) {
    super("browser signalling failed");
    this.name = "SipxTransportError";
    this.reason = reason;
  }
}
export class SipxSipError extends Error {
  constructor(status, reason = "request failed") {
    super("SIP request failed");
    this.name = "SipxSipError";
    this.status = status;
    this.reason = reason;
  }
}
export class SipxCancelled extends Error {
  constructor() {
    super("operation cancelled");
    this.name = "SipxCancelled";
  }
}
export class SipxCapabilityError extends Error {
  constructor(capability) {
    super(`this browser does not provide ${capability}`);
    this.name = "SipxCapabilityError";
    this.capability = capability;
  }
}
export function abiError(code) {
  const errors = contract.errors;
  const error =
    code === errors.State.code
      ? new SipxStateError()
      : [errors.Bounds.code, errors.Oom.code, errors.Limit.code].includes(code)
        ? new SipxLimitError()
        : new SipxAbiDefect(code);
  error.code = code;
  return error;
}
