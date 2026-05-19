export function cryptoRandomId(): string {
  const g = globalThis as any;
  if (g.crypto?.randomUUID) return g.crypto.randomUUID();
  return `id-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

export function cronScheduleForUser(userId: string): string {
  let h = 0;
  for (let i = 0; i < userId.length; i++) h = (h * 31 + userId.charCodeAt(i)) | 0;
  const offset = (h >>> 0) % 5;
  return `${offset}-59/5 * * * *`;
}

export const jsonReq = (method: string, body: unknown): RequestInit => ({
  method,
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify(body),
});

export const calId = (input: any): string =>
  encodeURIComponent(input?.calendarId ?? "primary");

export const eventsPath = (input: any): string =>
  `/calendars/${calId(input)}/events`;

export const eventPath = (input: any): string =>
  `${eventsPath(input)}/${encodeURIComponent(input.eventId)}`;
