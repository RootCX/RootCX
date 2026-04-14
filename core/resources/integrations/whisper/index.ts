/// <reference path="../rootcx-worker.d.ts" />

interface Config {
  apiKey?: string;
  baseUrl?: string;
  model?: string;
}

const DEFAULT_BASE_URL = "https://api.openai.com/v1";
const DEFAULT_MODEL = "whisper-1";

async function transcribe(config: Config, input: any) {
  const { url, language } = input;
  if (!url) throw new Error("missing required field: url");
  if (!config.apiKey) throw new Error("whisper integration not configured — apiKey required");

  // Download audio from the provided URL (typically a nonce download URL from Core storage)
  const audioRes = await fetch(url);
  if (!audioRes.ok) throw new Error(`failed to download audio: ${audioRes.status}`);
  const audioBytes = await audioRes.arrayBuffer();

  // Guess a filename from content-type for the multipart form
  const ct = audioRes.headers.get("content-type") ?? "audio/ogg";
  const ext = MIME_TO_EXT[ct] ?? "ogg";
  const filename = `audio.${ext}`;

  // Build multipart form — OpenAI Whisper API format
  const form = new FormData();
  form.append("file", new Blob([audioBytes], { type: ct }), filename);
  form.append("model", config.model ?? DEFAULT_MODEL);
  if (language) form.append("language", language);

  const baseUrl = (config.baseUrl ?? DEFAULT_BASE_URL).replace(/\/+$/, "");
  const res = await fetch(`${baseUrl}/audio/transcriptions`, {
    method: "POST",
    headers: { "Authorization": `Bearer ${config.apiKey}` },
    body: form,
  });

  if (!res.ok) {
    const body = await res.text();
    throw new Error(`whisper API error ${res.status}: ${body}`);
  }

  const data = await res.json();
  return { text: data.text ?? "" };
}

const MIME_TO_EXT: Record<string, string> = {
  "audio/ogg": "ogg",
  "audio/mpeg": "mp3",
  "audio/mp3": "mp3",
  "audio/wav": "wav",
  "audio/x-wav": "wav",
  "audio/mp4": "m4a",
  "audio/m4a": "m4a",
  "audio/webm": "webm",
  "audio/flac": "flac",
  "audio/x-flac": "flac",
};

const actions: Record<string, (c: Config, i: any) => Promise<any>> = {
  transcribe,
};

const rpcHandlers: Record<string, (params: any) => Promise<any>> = {
  async __bind(params) {
    const { config } = params;
    if (!config.apiKey) throw new Error("apiKey is required");
    // Validate connectivity by hitting the models endpoint (lightweight, no audio needed)
    const baseUrl = (config.baseUrl ?? DEFAULT_BASE_URL).replace(/\/+$/, "");
    const res = await fetch(`${baseUrl}/models`, {
      headers: { "Authorization": `Bearer ${config.apiKey}` },
    });
    if (!res.ok) throw new Error(`cannot reach Whisper API at ${baseUrl}: ${res.status}`);
    return { ok: true };
  },

  async __integration(params) {
    const { action, input, config } = params;
    const handler = actions[action];
    if (!handler) throw new Error(`unknown action: ${action}`);
    return handler(config, input);
  },
};

serve({ rpc: rpcHandlers });
