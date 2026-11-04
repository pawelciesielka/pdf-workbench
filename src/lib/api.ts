import { Channel, invoke } from "@tauri-apps/api/core";

export interface Conversation {
  id: number;
  documentHash: string | null;
  workspaceId: number | null;
  title: string;
  model: string;
  mode: "full" | "pages" | null;
  createdAt: number;
  updatedAt: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  costUsd: number;
  readOnly: boolean;
  replacedByEditAt: number | null;
}

export interface DocumentInfo {
  hash: string;
  name: string;
  path: string;
  size: number;
  pageCount: number | null;
  indexed: boolean;
  conversations: Conversation[];
  previousConversations: Conversation[];
}

export interface WorkspaceInfo {
  id: number;
  name: string;
  updatedAt: number;
  documents: { hash: string; name: string; path: string | null }[];
  conversations: Conversation[];
}

export interface RecentItem {
  path: string;
  name: string;
  openedAt: number;
  exists: boolean;
}

export interface Selection {
  page: number;
  text: string;
}

export interface CitationView {
  page: number | null;
  citedText: string;
  documentHash: string | null;
  documentName: string | null;
}

export interface Segment {
  text: string;
  citations: CitationView[];
}

export interface ToolView {
  name: string;
  input: Record<string, unknown>;
  ok: boolean;
  result: string;
}

export interface UserDisplay {
  text: string;
  selection: Selection | null;
  action: string | null;
}

export interface AssistantDisplay {
  segments: Segment[];
  stopReason: string | null;
  model: string;
  tools?: ToolView[];
}

export type MessageView =
  | { id: number; role: "user"; display: UserDisplay; createdAt: number }
  | { id: number; role: "assistant"; display: AssistantDisplay; createdAt: number };

export interface ToolCall {
  id: string;
  name: string;
  input: Record<string, unknown>;
}

export interface ToolOutcome {
  ok: boolean;
  text: string;
}

export type ChatEvent =
  | { type: "Started"; model: string }
  | { type: "BlockStart"; index: number }
  | { type: "TextDelta"; index: number; text: string }
  | { type: "Citation"; index: number; citation: CitationView }
  | { type: "Fallback"; toModel: string }
  | ({ type: "ToolCall" } & ToolCall)
  | { type: "Done"; stopReason: string | null; conversation: Conversation | null }
  | { type: "Error"; message: string };

export interface Settings {
  model: string;
  models: string[];
  apiKeySource: "keyring" | "env" | null;
}

export interface OcrLine {
  text: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Wywołanie z danymi binarnymi w treści i parametrami w nagłówku. */
function rawInvoke<T>(cmd: string, body: Uint8Array, meta: unknown): Promise<T> {
  return invoke<T>(cmd, body, { headers: { "x-meta": encodeURIComponent(JSON.stringify(meta ?? null)) } });
}

const bytes = (b: ArrayBuffer) => new Uint8Array(b);

export const api = {
  openDocument: (path: string) => invoke<DocumentInfo>("open_document", { path }),
  readDocument: (path: string) => invoke<ArrayBuffer>("read_document", { path }),
  saveDocument: (data: Uint8Array, meta: { path: string; parentHash: string | null; backup: boolean }) =>
    rawInvoke<{ document: DocumentInfo; backupPath: string | null }>("save_document", data, meta),
  rotatePages: (data: Uint8Array, pages: number[], degrees: number) => rawInvoke<ArrayBuffer>("rotate_pages", data, { pages, degrees }).then(bytes),
  rasterizePages: (body: Uint8Array, meta: { pdfLen: number; items: { page: number; offset: number; len: number; width: number; height: number }[] }) =>
    rawInvoke<ArrayBuffer>("rasterize_pages", body, meta).then(bytes),
  addTextLayer: (data: Uint8Array, pages: { page: number; lines: OcrLine[] }[]) => rawInvoke<ArrayBuffer>("add_text_layer", data, pages).then(bytes),
  ocrImage: (png: Uint8Array) => rawInvoke<{ language: string; lines: OcrLine[] }>("ocr_image", png, null),
  savePageTexts: (hash: string, pages: string[], force = false) => invoke<void>("save_page_texts", { hash, pages, force }),
  listRecent: () => invoke<RecentItem[]>("list_recent"),
  removeRecent: (path: string) => invoke<void>("remove_recent", { path }),
  createConversation: (scope: { hash?: string; workspaceId?: number }) =>
    invoke<Conversation>("create_conversation", { hash: scope.hash ?? null, workspaceId: scope.workspaceId ?? null }),
  continueConversation: (oldId: number, hash: string) => invoke<Conversation>("continue_conversation", { oldId, hash }),
  deleteConversation: (id: number) => invoke<void>("delete_conversation", { id }),
  getMessages: (conversationId: number) => invoke<MessageView[]>("get_messages", { conversationId }),
  sendMessage: (conversationId: number, text: string, selection: Selection | null, action: string | null, onEvent: (e: ChatEvent) => void) => {
    const channel = new Channel<ChatEvent>();
    channel.onmessage = onEvent;
    return invoke<void>("send_message", { conversationId, text, selection, action, onEvent: channel });
  },
  toolResult: (id: string, outcome: ToolOutcome) => invoke<void>("tool_result", { id, ok: outcome.ok, text: outcome.text }),
  stopGeneration: (conversationId: number) => invoke<void>("stop_generation", { conversationId }),
  listWorkspaces: () => invoke<WorkspaceInfo[]>("list_workspaces"),
  saveWorkspace: (id: number | null, name: string, hashes: string[]) => invoke<WorkspaceInfo>("save_workspace", { id, name, hashes }),
  deleteWorkspace: (id: number) => invoke<void>("delete_workspace", { id }),
  getSettings: () => invoke<Settings>("get_settings"),
  setModel: (model: string) => invoke<void>("set_model", { model }),
  setApiKey: (key: string) => invoke<void>("set_api_key", { key }),
  clearApiKey: () => invoke<void>("clear_api_key"),
  getInitialFiles: () => invoke<string[]>("get_initial_files"),
};

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}
