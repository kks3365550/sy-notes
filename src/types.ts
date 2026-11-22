export interface DocumentItem {
  id: string;
  title: string;
  content: string; // JSON string of BlockNote blocks
  content_text: string; // Plain text representation for FTS
  doc_type: "daily" | "memo" | "note";
  date?: string | null;
  created_at: number;
  updated_at: number;
  deleted_at: number | null;
}

export interface StorageInfo {
  db_path: string;
  is_fallback: boolean;
  is_portable: boolean;
}

/** Error returned by the `create_or_update_doc` command. */
export interface SaveError {
  code: "conflict" | "not_found" | "error";
  message: string;
  actual_updated_at: number | null;
}

export type ThemeMode = "light" | "dark";
