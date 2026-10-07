import { SocksClient } from "socks";

export function mailProxy(): string | undefined {
  if (process.env.SANDBOX_RUNTIME !== "1") return;
  let proxy: URL;
  try { proxy = new URL(process.env.ALL_PROXY ?? ""); }
  catch { throw new Error("Sandbox mail requires its local SOCKS proxy"); }
  if (!["http:", "socks5:", "socks5h:"].includes(proxy.protocol)
    || !["localhost", "127.0.0.1", "[::1]"].includes(proxy.hostname)
    || !proxy.port || proxy.search || proxy.hash || !["", "/"].includes(proxy.pathname)) {
    throw new Error("Sandbox mail requires its local SOCKS proxy");
  }
  // SRT's ALL_PROXY advertises HTTP, but its local multiplexer also speaks SOCKS5.
  const auth = proxy.username || proxy.password ? `${proxy.username}:${proxy.password}@` : "";
  proxy = new URL(`socks5://${auth}${proxy.hostname === "localhost" ? "127.0.0.1" : proxy.hostname}:${proxy.port}`);
  return proxy.href;
}

export async function imapProxySocket(host: string, port: number) {
  const proxyUrl = mailProxy();
  if (!proxyUrl) return;
  const proxy = new URL(proxyUrl);
  // ImapFlow's proxy option resolves the destination inside the sandbox. SOCKS
  // must receive the hostname so SRT resolves it and rejects private addresses.
  const { socket } = await SocksClient.createConnection({
    proxy: { host: proxy.hostname.replace(/^\[|\]$/g, ""), port: Number(proxy.port), type: 5,
      userId: decodeURIComponent(proxy.username), password: decodeURIComponent(proxy.password) },
    command: "connect", destination: { host, port }, timeout: 16_000,
  });
  return socket;
}
