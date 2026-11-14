export interface Position { line: number; character: number }
export interface Range { start: Position; end: Position }
export interface TextEdit { range: Range; text: string }
export interface Source { path: string; text: string; version?: number | null; uri?: string | null }
export interface WorkspaceInput { configuration: { root?: string; source_roots?: string[]; case_sensitive?: boolean; compiler_features?: string[]; resolved_configuration?: unknown; active_profile?: string | null; toolchain_identity?: string; invalid_inputs?: boolean }; sources: Source[]; aliases?: Record<string, string>; app_manifest?: string | null }
export interface Response<T> { protocol: 1; id: number; ok: true; revision: number; result: T }
export interface QueryResult<T> { document_version: number | null; value: T }
export interface RequestOptions { signal?: AbortSignal; workspaceRevision?: number }
export class EditorError extends Error { readonly code: string }
export interface Editor {
    readonly revision: number;
    request<T = unknown>(method: string, params: unknown, options?: RequestOptions): Promise<Response<T>>;
    openWorkspace(inputs: WorkspaceInput): Promise<Response<{ api_version: 1 }>>;
    updateConfiguration(config: WorkspaceInput['configuration']): Promise<Response<unknown>>;
    updateProviderSchema(text: string | null): Promise<Response<unknown>>;
    openDocument(source: Source): Promise<Response<unknown>>;
    updateDocument(path: string, edits: TextEdit[], version: number): Promise<Response<unknown>>;
    closeDocument(path: string): Promise<Response<unknown>>;
    removeFile(path: string): Promise<Response<unknown>>;
    query<T = unknown>(query: { kind: string; path?: string; position?: Position; range?: Range; [key: string]: unknown }, options?: RequestOptions): Promise<Response<QueryResult<T>>>;
    describe(params?: { query?: string; search?: string }): Promise<Response<unknown>>;
    dispose(): Promise<void>;
}
export function createEditor(options?: { worker?: Worker; wasmURL?: string | URL; onNotification?: (notification: unknown) => void }): Editor;
