#!/usr/bin/env node
// stdio ↔ HTTP bridge — auto-detects Streamable HTTP vs legacy SSE
import { createInterface } from 'node:readline'

const args = process.argv.slice(2)
const url = args.find(a => !a.startsWith('--'))
if (!url) { process.stderr.write('usage: mcp-bridge <url> [--header K:V]\n'); process.exit(1) }

const hdrs = {}
for (let i = 0; i < args.length; i++)
  if (args[i] === '--header' && args[++i]) {
    const j = args[i].indexOf(':')
    if (j > 0) hdrs[args[i].slice(0, j).trim()] = args[i].slice(j + 1).trim()
  }

let sid, mode, legacyEp

const log = m => process.stderr.write(`[mcp-bridge] ${m}\n`)

const emit = d => { try { JSON.parse(d); process.stdout.write(d + '\n') } catch {} }

const h = extra => {
  const o = { ...hdrs, ...extra }
  if (sid) o['Mcp-Session-Id'] = sid
  return o
}

async function parseSSE(body, cb) {
  const r = body.getReader(), d = new TextDecoder()
  let buf = '', evt = null, data = ''
  for (;;) {
    const { done, value } = await r.read()
    if (done) break
    buf += d.decode(value, { stream: true })
    let nl
    while ((nl = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, nl).trimEnd()
      buf = buf.slice(nl + 1)
      if (!line) { if (data) cb(evt, data.trim()); evt = null; data = ''; continue }
      if (line.startsWith('event:')) evt = line.slice(6).trim()
      else if (line.startsWith('data:')) data += (data ? '\n' : '') + line.slice(5).trimStart()
    }
  }
  if (data) cb(evt, data.trim())
}

async function readRes(res) {
  const s = res.headers.get('mcp-session-id')
  if (s) sid = s
  if (res.status === 202) return
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  if ((res.headers.get('content-type') || '').includes('text/event-stream'))
    await parseSSE(res.body, (_, d) => emit(d))
  else { const t = await res.text(); if (t.trim()) emit(t.trim()) }
}

const post = async line => readRes(await fetch(url, {
  method: 'POST', body: line,
  headers: h({ 'Content-Type': 'application/json', Accept: 'application/json, text/event-stream' }),
}))

function openGet() {
  fetch(url, { headers: h({ Accept: 'text/event-stream' }) })
    .then(r => r.ok && parseSSE(r.body, (_, d) => emit(d)))
    .catch(() => {})
}

async function legacyConnect() {
  const res = await fetch(url, { headers: { ...hdrs, Accept: 'text/event-stream' } })
  if (!res.ok) throw new Error(`legacy SSE: ${res.status}`)
  await parseSSE(res.body, (evt, d) => {
    if (evt === 'endpoint') legacyEp = new URL(d, url).href
    else emit(d)
  })
}

async function legacyPost(line) {
  while (!legacyEp) await new Promise(r => setTimeout(r, 50))
  const res = await fetch(legacyEp, {
    method: 'POST', body: line, headers: { ...hdrs, 'Content-Type': 'application/json' },
  })
  if (res.headers.get('content-type')?.includes('application/json')) {
    const t = await res.text(); if (t.trim()) emit(t.trim())
  }
}

async function fallback(line, reason) {
  mode = 'sse'
  log(reason ? `fallback legacy-sse (${reason})` : 'connected (legacy-sse)')
  legacyConnect().catch(e => { log(e.message); process.exit(1) })
  await legacyPost(line)
}

// first message: try Streamable HTTP, fallback to legacy SSE on 4xx
async function detect(line) {
  try {
    const res = await fetch(url, {
      method: 'POST', body: line,
      headers: { ...hdrs, 'Content-Type': 'application/json', Accept: 'application/json, text/event-stream' },
    })
    if (res.ok) { mode = 'http'; log('connected (streamable-http)'); await readRes(res); openGet(); return }
    if (res.status >= 400 && res.status < 500) return fallback(line)
    throw new Error(`HTTP ${res.status}`)
  } catch (e) { if (!mode) return fallback(line, e.message) }
}

async function send(line) {
  if (!mode) return detect(line)
  return mode === 'http' ? post(line) : legacyPost(line)
}

;(async () => {
  for await (const line of createInterface({ input: process.stdin })) {
    if (!line.trim()) continue
    try { await send(line) } catch (e) {
      try {
        const { id } = JSON.parse(line)
        if (id != null) emit(JSON.stringify({ jsonrpc: '2.0', id, error: { code: -32000, message: e.message } }))
      } catch {}
    }
  }
})().catch(() => process.exit(0))

process.stdin.on('end', () => {
  if (sid) fetch(url, { method: 'DELETE', headers: h() }).catch(() => {})
  setTimeout(() => process.exit(0), 100)
})
