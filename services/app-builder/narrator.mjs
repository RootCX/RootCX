import { contextText } from './narration-context.mjs';

const sentences = new Intl.Segmenter('fr', { granularity: 'sentence' });
const phases = new Set(['understanding', 'editing', 'checking', 'repairing', 'waiting']);

export function businessText(value) {
  if (typeof value !== 'string') return null;
  const text = value.trim();
  if (!text || text.length > 500 || /[`{}<>]|https?:|\b(?:rootcx|opencode|sql|postgres|typescript|javascript|npm|bun|vite|rebuilt|rebuild|typecheck|backend|frontend|migration|api|sdk|git|terminal|commit|deploy|déploiement|compil\w*)\b|(?:src|public|backend)\//i.test(text)) return null;
  return text;
}

// This Adapter never falls back to the coding endpoint or its model.
export function narrationGateway({ narrationEndpoint, apiKey }) {
  return async (input, signal) => {
    if (!narrationEndpoint) return null;
    const response = await fetch(narrationEndpoint, {
      method: 'POST', redirect: 'error', signal,
      headers: { 'content-type': 'application/json', 'x-api-key': apiKey },
      body: JSON.stringify(input),
    });
    if (!response.ok) throw Object.assign(new Error('Narration unavailable'), { status: response.status });
    const result = await response.json();
    return result.kind ? result : result.message;
  };
}

// Observe facts, never await presentation. The translator is the only provider Seam.
export function narrator({ translate, prompt, context = [], signal, onProgress }) {
  let phase = 'understanding';
  let revision = 0;
  let inFlight = false;
  let lastSent = -Infinity;
  let calls = 0;
  let closed = false;
  const previous = [];
  const pending = new Map();
  let active;
  let engineWorking = false;
  let lastActivity = Date.now();
  let lastReminder = Date.now();
  const observed = new Set();
  let lastObservation;
  const controller = new AbortController();
  const timer = setInterval(() => void flush(), 4000);
  timer.unref();

  async function flush() {
    if (closed || signal.aborted || inFlight || calls >= 40 || Date.now() - lastSent < 4000) return;
    if (!pending.size && (active?.status === 'running' || engineWorking) && Date.now() - Math.max(lastActivity, lastReminder) >= 20000) {
      pending.set('waiting', { ...(active?.status === 'running' ? active : { phase, action: 'work', source: 'engine', status: 'running', area: 'application' }), stillRunning: true, elapsedSeconds: Math.floor((Date.now() - lastActivity) / 1000) });
      lastReminder = Date.now();
    }
    if (!pending.size) return;
    inFlight = true;
    calls++;
    lastSent = Date.now();
    const observedRevision = revision;
    const facts = [...pending.values()].slice(-8);
    while (facts.map(fact => JSON.stringify(fact)).join('\n').length > 4000) facts.shift();
    const evidence = facts.map(fact => JSON.stringify(fact)).join('\n');
    pending.clear();
    try {
      const timeout = AbortSignal.any([signal, controller.signal, AbortSignal.timeout(3000)]);
      // Enforce the deadline even if a translator ignores cancellation.
      const cancelled = new Promise((_, reject) => {
        timeout.addEventListener('abort', () => reject(timeout.reason), { once: true });
      });
      const result = await Promise.race([translate({
        request: contextText(prompt, 2000), phase, observation: evidence,
        context: context.slice(-3),
        previousMessage: previous.join('\n').slice(-500),
      }, timeout), cancelled]);
      const message = businessText(typeof result === 'string' ? result : result?.message);
      if (typeof result === 'object' && result !== null && !['progress', 'verified'].includes(result.kind)) return;
      const verified = result?.kind === 'verified';
      const invalidated = facts.findLastIndex(fact => fact.status === 'error' || ['editing', 'repairing'].includes(fact.phase));
      const verificationAllowed = facts.slice(invalidated + 1).some(fact => fact.source === 'platform' && fact.status === 'completed' && ['test', 'typecheck', 'review'].includes(fact.action));
      if (verified && !verificationAllowed) return;
      if (!verified && message && /(?:\b(?:validé(?:e|s|es)?|vérifié(?:e|s|es)?|testé(?:e|s|es)?|succès|corrects?|conformes?|fonctionne|réussi[es]*)\b|tout est bon)/iu.test(message)) return;
      if (!closed && !signal.aborted && revision === observedRevision && message && message !== 'RIEN'
        && !previous.some(text => text.toLocaleLowerCase() === message.toLocaleLowerCase())
        && message.length <= 240 && [...sentences.segment(message)].length <= 1
        && !/(?:^|[^\p{L}])(?:termin[ée]e?s?|disponibles?|publi[ée]e?s?|appliqu[ée]e?s?|prêt[es]*|enregistr[ée]e?s?|publication|en ligne|en place|c’est fait|c'est fait)(?=$|[^\p{L}])/iu.test(message)) {
        previous.push(message);
        if (previous.length > 2) previous.shift();
        onProgress(phase, message);
      }
    } catch (error) {
      if ([401, 402, 403, 404].includes(error?.status)) calls = 40;
    } finally { inFlight = false; }
  }
  return {
    working(value) { engineWorking = value; },
    observe(fact) {
      if (closed || !phases.has(fact.phase)) return;
      const next = fact.phase;
      // A finite vocabulary bounds memory and prevents logs, paths or secrets entering the prompt.
      const status = ['pending', 'running', 'completed', 'error'].includes(fact.status) ? fact.status : 'observed';
      const area = ['screen', 'data', 'rules'].includes(fact.area) ? fact.area : 'application';
      const action = ['inspect', 'work', 'plan', 'edit', 'typecheck', 'test', 'build', 'dependencies', 'review'].includes(fact.action) ? fact.action : 'inspect';
      const source = fact.source === 'platform' ? 'platform' : 'tool';
      const intent = contextText(fact.intent, 240);
      const evidence = JSON.stringify({ phase: next, status, area, action, source, ...(intent ? { intent } : {}) });
      // Tool identity stays private; only bounded categories reach the translator.
      const key = fact.id ? `${fact.id}:${status}` : evidence;
      if (fact.id ? observed.has(key) : lastObservation === key) return;
      // Invalidate on a new action or outcome too, even if the phase is unchanged.
      phase = next; revision++;
      active = JSON.parse(evidence);
      lastActivity = Date.now();
      lastObservation = key;
      if (fact.id) {
        observed.add(key);
        if (observed.size > 128) observed.delete(observed.values().next().value);
      }
      const pendingKey = fact.id ?? `${source}:${action}`;
      pending.delete(pendingKey);
      pending.set(pendingKey, active);
      if (pending.size > 8) pending.delete(pending.keys().next().value);
      void flush();
    },
    close() { closed = true; clearInterval(timer); controller.abort(); },
  };
}
