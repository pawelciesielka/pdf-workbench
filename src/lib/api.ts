import { Channel, invoke } from "@tauri-apps/api/core";

export interface Conversation {
  id: number;
  documentHash: string;
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
}

export interface Segment {
  text: string;
  citations: CitationView[];
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
}

export type MessageView =
  | { id: number; role: "user"; display: UserDisplay; createdAt: number }
  | { id: number; role: "assistant"; display: AssistantDisplay; createdAt: number };

export type ChatEvent =
  | { type: "Started"; model: string }
  | { type: "BlockStart"; index: number }
  | { type: "TextDelta"; index: number; text: string }
  | { type: "Citation"; index: number; page: number | null; citedText: string }
  | { type: "Fallback"; toModel: string }
  | { type: "Done"; stopReason: string | null; conversation: Conversation | null }
  | { type: "Error"; message: string };

export interface Settings {
  model: string;
  models: string[];
  apiKeySource: "keyring" | "env" | null;
}

export const api = {
  openDocument: (path: string) => invoke<DocumentInfo>("open_document", { path }),
  readDocument: (path: string) => invoke<ArrayBuffer>("read_document", { path }),
  savePageTexts: (hash: string, pages: string[]) => invoke<void>("save_page_texts", { hash, pages }),
  listRecent: () => invoke<RecentItem[]>("list_recent"),
  removeRecent: (path: string) => invoke<void>("remove_recent", { path }),
  createConversation: (hash: string) => invoke<Conversation>("create_conversation", { hash }),
  deleteConversation: (id: number) => invoke<void>("delete_conversation", { id }),
  getMessages: (conversationId: number) => invoke<MessageView[]>("get_messages", { conversationId }),
  sendMessage: (
    conversationId: number,
    text: string,
    selection: Selection | null,
    action: string | null,
    onEvent: (e: ChatEvent) => void,
  ) => {
    const channel = new Channel<ChatEvent>();
    channel.onmessage = onEvent;
    return invoke<void>("send_message", { conversationId, text, selection, action, onEvent: channel });
  },
  stopGeneration: (conversationId: number) => invoke<void>("stop_generation", { conversationId }),
  getSettings: () => invoke<Settings>("get_settings"),
  setModel: (model: string) => invoke<void>("set_model", { model }),
  setApiKey: (key: string) => invoke<void>("set_api_key", { key }),
  clearApiKey: () => invoke<void>("clear_api_key"),
  getInitialFiles: () => invoke<string[]>("get_initial_files"),
};

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}
