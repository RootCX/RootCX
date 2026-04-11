const DOW = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const pad = (n: number) => String(n).padStart(2, "0");
const hhmm = (h: number, m: number) => `${pad(h)}:${pad(m)}`;

export function humanizeCron(schedule: string): string {
  const parts = schedule.trim().split(/\s+/);

  if (parts.length === 2 && parts[1] === "seconds") {
    const n = parseInt(parts[0], 10);
    return n === 1 ? "Every second" : `Every ${n} seconds`;
  }
  if (parts.length !== 5) return schedule;

  const [min, hour, dom, mon, dow] = parts;
  const starsOnly = (fs: string[]) => fs.every((f) => f === "*");
  const num = (s: string) => (/^\d+$/.test(s) ? parseInt(s, 10) : null);

  if (min === "*" && starsOnly([hour, dom, mon, dow])) return "Every minute";

  if (/^\*\/\d+$/.test(min) && starsOnly([hour, dom, mon, dow])) {
    const n = parseInt(min.slice(2), 10);
    return n === 1 ? "Every minute" : `Every ${n} minutes`;
  }

  if (min === "0" && /^\*\/\d+$/.test(hour) && starsOnly([dom, mon, dow])) {
    const n = parseInt(hour.slice(2), 10);
    return n === 1 ? "Every hour" : `Every ${n} hours`;
  }

  const m = num(min), h = num(hour), d = num(dom), w = num(dow);

  if (m !== null && hour === "*" && starsOnly([dom, mon, dow])) {
    return m === 0 ? "Every hour" : `Every hour at :${pad(m)}`;
  }

  if (m !== null && h !== null && starsOnly([dom, mon, dow])) {
    if (h === 0 && m === 0) return "Every day at midnight";
    return `Every day at ${hhmm(h, m)}`;
  }

  if (m !== null && h !== null && dom === "*" && mon === "*" && w !== null) {
    return `Every ${DOW[w] ?? `day ${w}`} at ${hhmm(h, m)}`;
  }

  if (m !== null && h !== null && d !== null && mon === "*" && dow === "*") {
    return `On day ${d} of every month at ${hhmm(h, m)}`;
  }

  if (m !== null && h !== null && dom === "$" && mon === "*" && dow === "*") {
    return `On the last day of every month at ${hhmm(h, m)}`;
  }

  return schedule;
}

export function formatDuration(start: string | null, end: string | null): string {
  if (!start || !end) return "—";
  const ms = new Date(end).getTime() - new Date(start).getTime();
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
  return `${(ms / 60000).toFixed(1)}m`;
}
