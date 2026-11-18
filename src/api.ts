export interface Counts {
  corePixels: number;
  unstablePixels: number;
  clampedPixels: number;
  outsideDiffPixels: number;
}

export interface Report {
  schemaVersion: number;
  input: { width: number; height: number; format: string };
  output: { format: string; lossy: boolean } | null;
  templateId: string | null;
  templateVersion: number | null;
  qualityClaim: "pixel" | "visual" | null;
  blendMode: string | null;
  colorSpace: string | null;
  bbox: [number, number, number, number] | null;
  shift: [number, number] | null;
  ncc: number | null;
  alphaMae: number | null;
  method: string;
  verdict:
    | "pass"
    | "pass_with_unstable"
    | "needs_review"
    | "no_watermark"
    | "unsupported"
    | "failed";
  flags: string[];
  counts: Counts;
  holdout: null | {
    psnr: number;
    ssimMin: number;
    maxAbs: number;
    stableMaxAbs: number;
  };
}

export interface Probe {
  bbox: [number, number, number, number];
  alphaBase64: string;
  unstableBase64: string;
}

export interface ProcessResponse {
  report: Report;
  imageBase64: string | null;
  probe: Probe | null;
  error?: string;
}

export interface SampleResponse {
  imageBase64: string;
  alphaBase64: string;
  templateJson: string;
  note: string;
}

export interface TemplateBundle {
  templateJson: string;
  alphaBase64: string;
}

async function inTauri(): Promise<boolean> {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

async function call<T>(command: string, path: string, body?: unknown): Promise<T> {
  if (await inTauri()) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>(command, body === undefined ? undefined : { body });
  }
  const response = await fetch(path, {
    method: body === undefined ? "GET" : "POST",
    headers: body === undefined ? undefined : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  const parsed = (text ? JSON.parse(text) : {}) as T & { error?: string };
  if (!response.ok) {
    throw new Error(parsed.error || `请求失败（${response.status}）`);
  }
  if (parsed.error) {
    throw new Error(parsed.error);
  }
  return parsed;
}

export function fetchSample(): Promise<SampleResponse> {
  return call("sample_image", "/api/sample");
}

export function processImage(body: {
  imageBase64: string;
  templateJson?: string;
  alphaBase64?: string;
  templates?: TemplateBundle[];
  accept?: boolean;
  jpeg?: boolean;
  noWatermark?: boolean;
  keepExif?: boolean;
  bbox?: [number, number, number, number];
}): Promise<ProcessResponse> {
  return call("process_image", "/api/process", body);
}

export function base64ToBlob(value: string, mime: string): Blob {
  const bytes = base64ToBytes(value);
  const buffer = new ArrayBuffer(bytes.byteLength);
  new Uint8Array(buffer).set(bytes);
  return new Blob([buffer], { type: mime });
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

export function base64ToBytes(value: string): Uint8Array {
  const binary = atob(value);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    out[i] = binary.charCodeAt(i);
  }
  return out;
}

export async function fileToBase64(file: File): Promise<string> {
  const buffer = await file.arrayBuffer();
  return bytesToBase64(new Uint8Array(buffer));
}

export function base64ToObjectUrl(value: string, mime = "image/png"): string {
  return URL.createObjectURL(base64ToBlob(value, mime));
}

export const VERDICT_LABEL: Record<Report["verdict"], string> = {
  pass: "通过",
  pass_with_unstable: "通过，含不稳定像素",
  needs_review: "需要复核",
  no_watermark: "无水印",
  unsupported: "未识别",
  failed: "失败",
};

export const COMPLIANCE =
  "本工具只用于处理你本人生成或已获得处理权的图片。请不要用它去除他人作品上的权利标识。导出图会改写水印区域的像素，并不会带走原图的内容凭证。能否达到像素级还原，以该模板的质量标记和每张图的验证报告为准。";
