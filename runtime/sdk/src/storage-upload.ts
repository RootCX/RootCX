export const DEFAULT_STORAGE_CHUNK_SIZE = 8 * 1024 * 1024;
export const DEFAULT_STORAGE_IDLE_TIMEOUT_MS = 60_000;

export interface StorageChunkRequest {
  url: string;
  token: string | null;
  offset: number;
  chunk: Blob;
  idleTimeoutMs: number;
  signal?: AbortSignal;
  onProgress?: (uploadedBytes: number) => void;
}

export interface StorageChunkResponse {
  status: number;
  body: string;
}

export function supportsUploadProgress(): boolean {
  return typeof XMLHttpRequest !== "undefined";
}

export function sendStorageChunk(request: StorageChunkRequest): Promise<StorageChunkResponse> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    let idleTimer: ReturnType<typeof setTimeout> | undefined;
    let transmittedBytes = 0;
    let stalled = false;
    let settled = false;

    function clearIdleTimer(): void {
      if (idleTimer !== undefined) clearTimeout(idleTimer);
    }

    function settle(action: () => void): void {
      if (settled) return;
      settled = true;
      clearIdleTimer();
      request.signal?.removeEventListener("abort", abortFromSignal);
      action();
    }

    function abortFromSignal(): void {
      xhr.abort();
    }

    function resetIdleTimer(): void {
      clearIdleTimer();
      idleTimer = setTimeout(() => {
        stalled = true;
        xhr.abort();
      }, request.idleTimeoutMs);
    }

    xhr.upload.addEventListener("progress", (event) => {
      const nextTransmittedBytes = Math.min(event.loaded, request.chunk.size);
      if (nextTransmittedBytes <= transmittedBytes) return;
      transmittedBytes = nextTransmittedBytes;
      resetIdleTimer();
      request.onProgress?.(transmittedBytes);
    });
    xhr.addEventListener("load", () => {
      settle(() => resolve({ status: xhr.status, body: xhr.responseText }));
    });
    xhr.addEventListener("error", () => {
      settle(() => reject(new TypeError("storage upload network error")));
    });
    xhr.addEventListener("abort", () => {
      settle(() => {
        if (stalled) {
          reject(new Error(`storage upload stalled for ${request.idleTimeoutMs} ms`));
          return;
        }
        reject(request.signal?.reason ?? new Error("storage upload aborted"));
      });
    });

    if (request.signal?.aborted) {
      settle(() => reject(request.signal?.reason ?? new Error("storage upload aborted")));
      return;
    }
    request.signal?.addEventListener("abort", abortFromSignal, { once: true });

    xhr.open("PATCH", request.url, true);
    xhr.setRequestHeader("Content-Type", "application/offset+octet-stream");
    xhr.setRequestHeader("Upload-Offset", String(request.offset));
    if (request.token) xhr.setRequestHeader("Authorization", `Bearer ${request.token}`);
    resetIdleTimer();
    xhr.send(request.chunk);
  });
}
