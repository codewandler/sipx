export type {
  ABI,
  Command,
  KernelEvent,
  MediaFacts,
  CodecFact,
  DialogFact,
  AbiErrorCode,
} from "./generated/wire.js";
import type { MediaFacts } from "./generated/wire.js";
export type ClientState =
  | "starting"
  | "connected"
  | "registered"
  | "closing"
  | "closed";
export type CallState =
  | "dialing"
  | "ringing"
  | "incoming"
  | "answering"
  | "established"
  | "ending"
  | "ended"
  | "failed";
export interface Configuration {
  v?: 1;
  aor: string;
  auth: { username: string; password: string };
  transport: {
    scheme: "wss" | "ws";
    host: string;

    resource: string;
  };
  insecure?: "refuse" | "allow-development";
}
export interface CreateOptions {
  wasmURL?: string | URL;
  setupTimeoutMs?: number;
  operationTimeoutMs?: number;
  onDiagnostic?: (event: { kind: string; call?: number }) => void;
}
export interface NegotiatedMedia {
  kernel?: MediaFacts;
  browser?: {
    connection_state?: string;
    dtls_state?: string;
    srtp_cipher?: string;
    selected_candidate_pair?: {
      id: string;
      local_candidate_id?: string;
      remote_candidate_id?: string;
      nominated?: boolean;
    };
    inbound?: { packets?: number; bytes?: number };
    outbound?: { packets?: number; bytes?: number };
  };
}
export declare class SipxClient {
  static create(
    configuration: Configuration,
    options?: CreateOptions,
  ): Promise<SipxClient>;
  private constructor();
  readonly state: ClientState;
  readonly calls: readonly SipxCall[];
  readonly diagnostics: Readonly<{
    listener_errors: number;
    dropped_after_close: number;
    unknown_events: number;
    transport: Readonly<Record<string, number>>;
  }>;
  on(event: "state", listener: (state: ClientState) => void): () => void;
  on(
    event: "incoming" | "outgoing",
    listener: (call: SipxCall) => void,
  ): () => void;
  on(event: "closed", listener: () => void): () => void;
  register(options?: { expires?: number; signal?: AbortSignal }): Promise<void>;
  unregister(): Promise<void>;
  dial(target: string, options?: { signal?: AbortSignal }): Promise<SipxCall>;
  setMicrophone(deviceId?: string): void;
  close(options?: { timeoutMs?: number }): Promise<void>;
}
export declare class SipxCall {
  private constructor();
  readonly id: number;
  readonly direction: "in" | "out";
  readonly from?: string;
  readonly to?: string;
  readonly state: CallState;
  on(event: "state", listener: (state: CallState) => void): () => void;
  on(
    event: "ended",
    listener: (cause: { cause: string; status?: number }) => void,
  ): () => void;
  ring(): Promise<void>;
  answer(options?: { signal?: AbortSignal }): Promise<SipxCall>;
  reject(status?: number): Promise<void>;
  hangup(): Promise<void>;
  mute(muted?: boolean): void;
  negotiatedMedia(): NegotiatedMedia | undefined;
  refreshStats(): Promise<NegotiatedMedia | undefined>;
}
export declare class SipxAbiDefect extends Error {
  readonly code?: number;
}
export declare class SipxStateError extends Error {}
export declare class SipxLimitError extends Error {
  readonly kind: string;
}
export declare class SipxTransportError extends Error {
  readonly reason: string;
}
export declare class SipxSipError extends Error {
  readonly status?: number;
  readonly reason: string;
}
export declare class SipxCancelled extends Error {}
export declare class SipxCapabilityError extends Error {
  readonly capability: string;
}
export declare class SipxMediaError extends Error {
  readonly kind:
    | "permission"
    | "device"
    | "autoplay"
    | "negotiation"
    | "track-ended";
}
export declare const experimental: true;
