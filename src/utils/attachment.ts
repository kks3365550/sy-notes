import { saveAttachment } from "../api";

export async function uploadImageAttachment(file: File): Promise<string> {
  const extension = file.name.split(".").pop() || "png";
  const uuid = typeof crypto !== "undefined" && crypto.randomUUID ? crypto.randomUUID() : `img-${Date.now()}`;
  const fileName = `${uuid}.${extension}`;

  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = async () => {
      try {
        const result = reader.result as string;
        // Strip base64 prefix
        const base64Data = result.includes(",") ? result.split(",")[1] : result;
        const filePath = await saveAttachment(fileName, base64Data);
        resolve(filePath);
      } catch (err) {
        console.error("Failed to upload image attachment:", err);
        // Fallback to blob URL so UI doesn't crash
        resolve(URL.createObjectURL(file));
      }
    };
    reader.onerror = (err) => reject(err);
    reader.readAsDataURL(file);
  });
}
