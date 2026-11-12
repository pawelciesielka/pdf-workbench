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

/** Obiekt treści strony (PDFium), współrzędne PDF [x0, y0, x1, y1]. */
export interface PageObject {
  index: number;
  kind: "text" | "image" | "path" | "shading" | "form";
  rect: [number, number, number, number];
  text?: string;
  fontSize?: number;
  fontName?: string;
  color?: string;
}

export type EditOp =
  | { op: "move"; page: number; index: number; dx: number; dy: number }
  | { op: "resize"; page: number; index: number; rect: [number, number, number, number] }
  | { op: "delete"; page: number; index: number }
  | { op: "setText"; page: number; index: number; text: string; fontSize?: number; color?: string }
  | { op: "addText"; page: number; x: number; y: number; text: string; fontSize: number; color?: string; bold?: boolean };

export interface EditReport {
  count: number;
  fontFallbacks: string[];
}

/** Podpis elektroniczny znaleziony w pliku. */
export interface SignatureInfo {
  kind: "signature" | "timestamp";
  signer: string | null;
  issuer: string | null;
  /** Data z pliku w formacie PDF („D:20261106143205+01'00'”). */
  time: string | null;
  reason: string | null;
  /** Podpis zgadza się z podpisanymi bajtami. */
  intact: boolean;
  /** Po podpisie nic nie dopisano do pliku. */
  coversWhole: boolean;
  timestamped: boolean;
  qualified: boolean;
  problem: string | null;
}

/** Certyfikat z kluczem prywatnym w magazynie Windows (np. z SimplySign Desktop). */
export interface SigningCert {
  id: string;
  name: string;
  issuer: string;
  validFrom: number;
  validTo: number;
  qualified: boolean;
}

export interface SignOptions {
  certId: string;
  signingTime: string;
  reason?: string;
  location?: string;
  timestamp: boolean;
  /** Dane do długoterminowej weryfikacji i znacznik czasu dokumentu (PAdES-LTA). */
  ltv: boolean;
  visible?: { page: number; rect: [number, number, number, number]; lines: string[] };
}

export interface SignReport {
  level: "B-B" | "B-T" | "B-LTA";
  warning: string | null;
}

/** Zaufanie do podpisu (w kolejności `signatures`). */
export interface TrustInfo {
  chainTrusted: boolean;
  chain: string[];
  revocation: "good" | "revoked" | "unknown";
  euQualified: boolean;
  euService: string | null;
  trustedTime: number | null;
  timestampQualified: boolean;
  ltv: boolean;
  notes: string[];
}

/** Miejsce widocznego podpisu: strona (100000 = ostatnia) i prostokąt w punktach PDF. */
export interface Placement {
  page: number;
  rect: [number, number, number, number];
}

export interface BatchItem {
  path: string;
  output: string | null;
  level: SignReport["level"] | null;
  warning: string | null;
  error: string | null;
}

/** Odpowiedź [u32 LE długość JSON][JSON][PDF]. */
function unpackReport<T>(buf: ArrayBuffer): { data: Uint8Array; report: T } {
  const all = new Uint8Array(buf);
  const len = new DataView(buf).getUint32(0, true);
  return { report: JSON.parse(new TextDecoder().decode(all.subarray(4, 4 + len))) as T, data: all.slice(4 + len) };
}

/** Odpowiedź [u32 LE długość JSON][JSON][PDF]. */
function unpackEdit(buf: ArrayBuffer): { data: Uint8Array; report: EditReport } {
  const all = new Uint8Array(buf);
  const len = new DataView(buf).getUint32(0, true);
  const report = JSON.parse(new TextDecoder().decode(all.subarray(4, 4 + len))) as EditReport;
  return { data: all.slice(4 + len), report };
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
  signatures: (data: Uint8Array) => rawInvoke<SignatureInfo[]>("pdf_signatures", data, null),
  signingCerts: () => invoke<SigningCert[]>("signing_certs"),
  signPdf: (data: Uint8Array, o: SignOptions) => rawInvoke<ArrayBuffer>("sign_pdf", data, o).then((b) => unpackReport<SignReport>(b)),
  signatureTrust: (data: Uint8Array) => rawInvoke<TrustInfo[]>("signature_trust", data, null),
  signFiles: (options: { paths: string[]; sign: SignOptions; visibleLines: string[] | null; placements: (Placement | null)[] }, onProgress: (index: number, item: BatchItem) => void) => {
    const channel = new Channel<{ index: number; item: BatchItem }>();
    channel.onmessage = (m) => onProgress(m.index, m.item);
    return invoke<BatchItem[]>("sign_files", { options, onProgress: channel });
  },
  rotatePages: (data: Uint8Array, pages: number[], degrees: number) => rawInvoke<ArrayBuffer>("rotate_pages", data, { pages, degrees }).then(bytes),
  rasterizePages: (body: Uint8Array, meta: { pdfLen: number; items: { page: number; offset: number; len: number; width: number; height: number }[] }) =>
    rawInvoke<ArrayBuffer>("rasterize_pages", body, meta).then(bytes),
  addTextLayer: (data: Uint8Array, pages: { page: number; lines: OcrLine[] }[]) => rawInvoke<ArrayBuffer>("add_text_layer", data, pages).then(bytes),
  pageObjects: (data: Uint8Array, page: number) => rawInvoke<PageObject[]>("page_objects", data, { page }),
  editObjects: (data: Uint8Array, ops: EditOp[]) => rawInvoke<ArrayBuffer>("edit_objects", data, ops).then(unpackEdit),
  replaceText: (data: Uint8Array, m: { pages?: number[]; find: string; replace: string; within?: [number, number, number, number] }) =>
    rawInvoke<ArrayBuffer>("replace_text", data, m).then(unpackEdit),
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
  gmailStatus: () => invoke<GmailStatus>("gmail_status"),
  gmailSetClient: (clientId: string, clientSecret: string) => invoke<void>("gmail_set_client", { clientId, clientSecret }),
  gmailLoadClientFile: (path: string) => invoke<void>("gmail_load_client_file", { path }),
  gmailClearClient: () => invoke<void>("gmail_clear_client"),
  gmailConnect: () => invoke<string>("gmail_connect"),
  gmailDisconnect: () => invoke<void>("gmail_disconnect"),
  gmailSetOptions: (query: string, doneLabel: string) => invoke<void>("gmail_set_options", { query, doneLabel }),
  gmailList: (orgOnly: boolean) => invoke<MailItem[]>("gmail_list", { orgOnly }),
  gmailDownload: (id: string, partIds: string[]) => invoke<string[]>("gmail_download", { id, partIds }),
  gmailReplyDraft: (id: string, files: string[], text: string) => invoke<DraftResult>("gmail_reply_draft", { id, files, text }),
  gmailSendDraft: (draftId: string) => invoke<string>("gmail_send_draft", { draftId }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  openLink: (url: string) => invoke<void>("open_link", { url }),
};

export interface GmailStatus {
  builtin: boolean;
  clientId: string | null;
  hasSecret: boolean;
  email: string | null;
  connected: boolean;
  query: string;
  doneLabel: string;
  folder: string;
}

export interface MailItem {
  id: string;
  threadId: string;
  from: string;
  to: string;
  cc: string;
  subject: string;
  date: number;
  snippet: string;
  unread: boolean;
  attachments: { partId: string; filename: string; size: number }[];
}

export interface DraftResult {
  draftId: string;
  messageId: string;
  to: string[];
  cc: string[];
  url: string;
  labelWarning: string | null;
}

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}
