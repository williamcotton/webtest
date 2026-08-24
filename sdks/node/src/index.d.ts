export type Transferable = null | boolean | number | string | Transferable[] | { [key: string]: Transferable };
export interface FunctionSchema { documentation?: string; retry_safe?: boolean; params: TypeSchema; returns: TypeSchema }
export type TypeSchema = { type: string; [key: string]: unknown };
export interface Manifest { manifest_version: 1; protocol: 1; provider: "app"; sdk: string; sdk_version: string; schema_hash: string; functions: Record<string, FunctionSchema> }
export const SDK_INFO: Readonly<{ name: "@webtest/node"; version: "0.1.0"; minimumProtocol: 1; maximumProtocol: 1; generatedSchemaRevision: 1; transports: readonly ["unix", "named_pipe", "tcp", "stdio"] }>;
export type Handler = (arguments_: Record<string, Transferable>, context: { signal: AbortSignal; emit(kind: string, value: Transferable): void }) => Transferable | Promise<Transferable>;
export class AppBridge {
  constructor(manifest: Manifest, options?: { maxMessageBytes?: number; maxPendingCalls?: number; maxEventsPerCall?: number; logger?: (message: string) => void });
  register<Arguments extends Record<string, Transferable>, Result extends Transferable>(
    name: string,
    handler: (arguments_: Arguments, context: { signal: AbortSignal; emit(kind: string, value: Transferable): void }) => Result | Promise<Result>,
  ): this;
  exportSchema(path: string): void;
  connectFromEnv(): Promise<void>;
}
