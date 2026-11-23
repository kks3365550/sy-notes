import { invoke } from "@tauri-apps/api/core";
import { DocumentItem, SaveError, StorageInfo } from "./types";

export async function getOrCreateDaily(dateStr?: string): Promise<DocumentItem> {
  const targetDate = dateStr || new Date().toISOString().split("T")[0];
  try {
    return await invoke<DocumentItem>("get_or_create_daily", { date: targetDate });
  } catch (err) {
    console.warn("invoke get_or_create_daily fallback:", err);
    const now = Date.now();
    return {
      id: `daily-${targetDate}`,
      title: `${targetDate} 일지`,
      content: JSON.stringify([
        {
          id: "daily-h1",
          type: "heading",
          props: { level: 1 },
          content: [{ type: "text", text: `${targetDate} 업무일지`, styles: {} }],
          children: [],
        },
        {
          id: "daily-check1",
          type: "checkListItem",
          props: { checked: false },
          content: [{ type: "text", text: "오늘의 주요 업무 계획 수립", styles: {} }],
          children: [],
        },
      ]),
      content_text: `${targetDate} 업무일지\n오늘의 주요 업무 계획 수립`,
      doc_type: "daily",
      date: targetDate,
      created_at: now,
      updated_at: now,
      deleted_at: null,
    };
  }
}

export async function getDoc(id: string): Promise<DocumentItem | null> {
  try {
    return await invoke<DocumentItem>("get_doc", { id });
  } catch (err) {
    console.warn("invoke get_doc fallback:", err);
    return null;
  }
}

/**
 * Saves a document. `doc.updated_at` must be the version last returned by the
 * backend (0 to skip the optimistic-lock check). Resolves with the committed
 * document (including its new `updated_at`).
 *
 * Unlike the other helpers this never swallows errors: a failed save rejects with
 * a {@link SaveError} so the UI can show it instead of claiming "저장됨".
 */
export async function createOrUpdateDoc(doc: DocumentItem): Promise<DocumentItem> {
  try {
    return await invoke<DocumentItem>("create_or_update_doc", { doc });
  } catch (err) {
    throw toSaveError(err);
  }
}

export function toSaveError(err: unknown): SaveError {
  if (err && typeof err === "object" && "code" in err) {
    const e = err as Partial<SaveError>;
    return {
      code: e.code === "conflict" || e.code === "not_found" ? e.code : "error",
      message: String(e.message ?? "저장 실패"),
      actual_updated_at: e.actual_updated_at ?? null,
    };
  }
  return { code: "error", message: String(err), actual_updated_at: null };
}

export async function deleteDoc(id: string): Promise<void> {
  try {
    await invoke("delete_doc", { id });
  } catch (err) {
    console.warn("invoke delete_doc fallback:", err);
  }
}

export async function restoreDoc(id: string): Promise<void> {
  try {
    await invoke("restore_doc", { id });
  } catch (err) {
    console.warn("invoke restore_doc fallback:", err);
  }
}

export interface PurgeReport {
  purged_docs: number;
  removed_attachments: number;
}

/**
 * Permanently deletes trashed documents and unreferenced attachment files.
 * `olderThanDays = null` empties the whole trash (confirm with the user first);
 * a number only removes documents trashed longer ago than that.
 * Rejects on failure so the UI can report it.
 */
export async function purgeTrash(olderThanDays: number | null = null): Promise<PurgeReport> {
  return await invoke<PurgeReport>("purge_trash", { olderThanDays });
}

export async function listDocs(includeDeleted = false): Promise<DocumentItem[]> {
  try {
    return await invoke<DocumentItem[]>("list_docs", { includeDeleted });
  } catch (err) {
    console.warn("invoke list_docs fallback:", err);
    return [];
  }
}

export async function listTrash(): Promise<DocumentItem[]> {
  try {
    return await invoke<DocumentItem[]>("list_trash");
  } catch (err) {
    console.warn("invoke list_trash fallback:", err);
    return [];
  }
}

export async function searchDocs(query: string, limit = 20): Promise<DocumentItem[]> {
  try {
    return await invoke<DocumentItem[]>("search_docs", { query, limit });
  } catch (err) {
    console.warn("invoke search_docs fallback:", err);
    return [];
  }
}

export async function saveAttachment(fileName: string, dataBase64: string): Promise<string> {
  try {
    return await invoke<string>("save_attachment", { fileName, dataBase64 });
  } catch (err) {
    console.warn("invoke save_attachment fallback:", err);
    return `data:image/png;base64,${dataBase64}`;
  }
}

export interface BackupReport {
  daily_path: string;
  weekly_path: string | null;
  bytes_copied: number;
}

export interface ExportReport {
  export_path: string;
  total_exported: number;
  active_count: number;
  trash_count: number;
  attachments_count: number;
  zip_bytes: number;
}

/** Creates a rotating DB snapshot now (7 daily + 4 weekly). Rejects on failure. */
export async function backupDb(): Promise<BackupReport> {
  return await invoke<BackupReport>("backup_db", { backupDir: null });
}

/** Exports every document and attachment to a ZIP under data/exports. Rejects on failure. */
export async function exportAll(): Promise<ExportReport> {
  return await invoke<ExportReport>("export_all", { exportPath: null });
}

export async function getStorageInfo(): Promise<StorageInfo> {
  try {
    return await invoke<StorageInfo>("get_storage_info");
  } catch (err) {
    return {
      db_path: "data/sy_notes.db",
      is_fallback: false,
      is_portable: true,
    };
  }
}

export interface AiConfig {
  /** OpenAI-compatible base URL, e.g. http://127.0.0.1:11434/v1 */
  endpoint: string;
  model: string;
}

/** Reads the AI server settings (persisted in the `settings` table). */
export async function getAiConfig(): Promise<AiConfig> {
  return await invoke<AiConfig>("get_ai_config");
}

/** Validates and persists the AI server settings. Rejects with a message on invalid input. */
export async function setAiConfig(config: Partial<AiConfig>): Promise<void> {
  await invoke("set_ai_config", { endpoint: config.endpoint, model: config.model });
}
