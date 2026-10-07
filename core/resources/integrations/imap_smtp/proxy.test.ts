import { afterAll, beforeAll, expect, test } from "bun:test";
import { createServer, connect, type Socket } from "node:net";
import { createServer as createTlsServer } from "node:tls";
import { fork, type ChildProcess } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { mailProxy } from "./sandbox-proxy";

const directory = mkdtempSync(join(tmpdir(), "rootcx-mail-proxy-"));
const sockets = new Set<Socket>();
const destinations: Array<{ host: string; port: number }> = [];
let imap: ReturnType<typeof createTlsServer>, proxy: ReturnType<typeof createServer>;
let smtp: ChildProcess;
let deliveries = 0;

function track(socket: Socket) {
  sockets.add(socket); socket.on("close", () => sockets.delete(socket)); socket.on("error", () => {});
  return socket;
}
async function listen(server: ReturnType<typeof createServer>) {
  await new Promise<void>((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  return (server.address() as { port: number }).port;
}

beforeAll(async () => {
  const cert = join(directory, "cert.pem"), key = join(directory, "key.pem");
  const result = Bun.spawnSync(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-subj", "/CN=mail.sandbox.invalid", "-addext", "subjectAltName=DNS:imap.sandbox.invalid,DNS:smtp.sandbox.invalid", "-keyout", key, "-out", cert]);
  expect(result.exitCode).toBe(0);
  const tls = { cert: readFileSync(cert), key: readFileSync(key) };
  imap = createTlsServer(tls, socket => {
    track(socket); socket.write("* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN] Test IMAP\r\n");
    let buffer = "";
    socket.on("data", data => {
      buffer += data.toString();
      while (buffer.includes("\r\n")) {
        const end = buffer.indexOf("\r\n"), line = buffer.slice(0, end); buffer = buffer.slice(end + 2);
        const [tag, verb] = line.split(" ");
        if (verb === "CAPABILITY") socket.write("* CAPABILITY IMAP4rev1 AUTH=PLAIN\r\n");
        if (verb === "LIST" || verb === "LSUB") socket.write(`* ${verb} (\\Inbox) "/" "INBOX"\r\n`);
        if (verb === "LOGOUT") { socket.end(`* BYE\r\n${tag} OK logout\r\n`); return; }
        socket.write(`${tag} OK ${verb}\r\n`);
      }
    });
  });
  // Node hosts the synthetic STARTTLS server; the application under test still runs in Bun.
  smtp = fork(new URL("./smtp-fixture.mjs", import.meta.url), [cert, key], { execPath: "node", stdio: ["ignore", "ignore", "inherit", "ipc"] });
  const smtpPort = await new Promise<number>((resolve, reject) => {
    smtp.on("message", (message: any) => { if (message.port) resolve(message.port); if (message.delivered) deliveries++; });
    smtp.once("error", reject);
  });
  const imapPort = await listen(imap);
  proxy = createServer(socket => {
    track(socket);
    socket.once("data", greeting => {
      if (greeting[0] !== 5) { socket.destroy(); return; }
      socket.write(Buffer.from([5, 2]));
      socket.once("data", auth => {
        const length = auth[1], username = auth.subarray(2, 2 + length).toString();
        const password = auth.subarray(3 + length).toString();
        if (username !== "sandbox-user" || password !== "synthetic-proxy") { socket.end(Buffer.from([1, 1])); return; }
        socket.write(Buffer.from([1, 0]));
        socket.once("data", request => {
        if (request[3] !== 3) { socket.destroy(); return; }
        const size = request[4], host = request.subarray(5, 5 + size).toString(), port = request.readUInt16BE(5 + size);
        destinations.push({ host, port });
        const target = host === "imap.sandbox.invalid" && port === 993 ? imapPort : host === "smtp.sandbox.invalid" && port === 587 ? smtpPort : undefined;
        if (!target) { socket.end(Buffer.from([5, 2, 0, 1, 0, 0, 0, 0, 0, 0])); return; }
        const upstream = track(connect(target, "127.0.0.1", () => {
          socket.write(Buffer.from([5, 0, 0, 1, 127, 0, 0, 1, 0, 0])); socket.pipe(upstream); upstream.pipe(socket);
        }));
        socket.on("close", () => upstream.destroy()); upstream.on("close", () => socket.destroy());
        });
      });
    });
  });
  await listen(proxy);
});

afterAll(async () => {
  for (const socket of sockets) socket.destroy();
  smtp?.kill();
  await Promise.all([imap, proxy].filter(Boolean).map(server => new Promise<void>(resolve => server.close(() => resolve()))));
  rmSync(directory, { recursive: true, force: true });
});

async function run(action: "get_folders" | "send_email", trust = true, proxyValue?: string) {
  const entry = new URL("./index.ts", import.meta.url).href;
  const code = `let rpc; globalThis.serve = value => { rpc = value.rpc }; globalThis.log = { warn() {} }; await import(${JSON.stringify(entry)}); const result = await rpc.__integration({action:${JSON.stringify(action)}, input:{to:"recipient@example.invalid",subject:"Synthetic test",body:"No real delivery"}, userCredentials:{imapHost:"imap.sandbox.invalid",imapPort:993,smtpHost:"smtp.sandbox.invalid",username:"test",password:"synthetic"}},null,{ action:async()=>{throw Error("unused")}}); console.log(JSON.stringify(result));`;
  const child = Bun.spawn([process.execPath, "-e", code], {
    cwd: import.meta.dir, env: {
      PATH: process.env.PATH, SANDBOX_RUNTIME: "1",
      ALL_PROXY: proxyValue ?? `http://sandbox-user:synthetic%2Dproxy@127.0.0.1:${(proxy.address() as { port: number }).port}`,
      ...(trust ? { NODE_EXTRA_CA_CERTS: join(directory, "cert.pem") } : {}),
    }, stdout: "pipe", stderr: "pipe",
  });
  const timer = setTimeout(() => child.kill(), 10_000);
  const [status, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  clearTimeout(timer);
  return { status, stdout, stderr };
}

test("sandbox mail keeps remote DNS, verified IMAP TLS and SMTP STARTTLS through SOCKS", async () => {
  for (const action of ["get_folders", "send_email"] as const) {
    const result = await run(action);
    expect(result.stderr).toBe(""); expect(result.status).toBe(0);
    expect(JSON.parse(result.stdout).ok).toBe(true);
  }
  expect(destinations).toContainEqual({ host: "imap.sandbox.invalid", port: 993 });
  expect(destinations).toContainEqual({ host: "smtp.sandbox.invalid", port: 587 });
  expect(deliveries).toBe(1);
}, 25_000);

test("sandbox mail fails closed on an untrusted certificate or refused proxy", async () => {
  const before = deliveries;
  for (const action of ["get_folders", "send_email"] as const) {
    for (const trust of [false, true]) {
      const result = await run(action, trust, trust ? "http://127.0.0.1:1" : undefined);
      expect(result.status).not.toBe(0);
    }
  }
  expect(deliveries).toBe(before);
}, 25_000);

test("sandbox cannot silently discard an absent or nonlocal proxy; legacy ignores proxy env", () => {
  const saved = { SANDBOX_RUNTIME: process.env.SANDBOX_RUNTIME, ALL_PROXY: process.env.ALL_PROXY };
  try {
    process.env.SANDBOX_RUNTIME = "1";
    for (const value of ["", "bad", "https://127.0.0.1:1080", "socks5://example.com:1080", "http://127.0.0.1", "http://127.0.0.1:1080/path"]) {
      process.env.ALL_PROXY = value; expect(() => mailProxy()).toThrow("Sandbox mail requires");
    }
    delete process.env.SANDBOX_RUNTIME;
    expect(mailProxy()).toBeUndefined();
  } finally {
    for (const [key, value] of Object.entries(saved)) if (value === undefined) delete process.env[key]; else process.env[key] = value;
  }
});
