import type * as Monaco from 'monaco-editor';
import type { Editor } from './index.js';
export function registerWebTest(monaco: typeof Monaco, editor: Editor, options?: { ownsModel?: (model: Monaco.editor.ITextModel) => boolean; onError?: (error: unknown) => void }): { refresh(): void; dispose(): void };
