import { invoke } from "@tauri-apps/api/core";
import { DocumentItem, StorageInfo } from "./types";

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

export async function createOrUpdateDoc(doc: DocumentItem): Promise<DocumentItem> {
  try {
    return await invoke<DocumentItem>("create_or_update_doc", { doc });
  } catch (err) {
    console.warn("invoke create_or_update_doc fallback:", err);
    return doc;
  }
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

export async function purgeTrash(olderThanDays = 30): Promise<number> {
  try {
    return await invoke<number>("purge_trash", { olderThanDays });
  } catch (err) {
    console.warn("invoke purge_trash fallback:", err);
    return 0;
  }
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
