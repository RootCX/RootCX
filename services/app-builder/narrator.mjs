const sentences = new Intl.Segmenter('fr', { granularity: 'sentence' });
const phases = new Set(['understanding', 'editing', 'checking', 'repairing', 'waiting']);

export function businessText(value) {
  if (typeof value !== 'string') return null;
  const text = value.trim();
  if (!text || text.length > 500 || /[`{}<>]|https?:|\b(?:rootcx|opencode|sql|postgres|typescript|javascript|npm|bun|vite|backend|frontend|migration|api|sdk|git|terminal|commit|deploy|déploiement|compil\w*)\b|(?:src|public|backend)\//i.test(text)) return null;
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
    return (await response.json()).message;
  };
}

// Observe facts, never await presentation. The translator is the only provider Seam.
export function narrator({ translate, prompt, signal, onProgress }) {
  let phase = 'understanding';
  let revision = 0;
  let inFlight = false;
  let lastSent = -Infinity;
  let calls = 0;
  let closed = false;
  const previous = [];
  const pending = new Set();
  const observed = new Set();
  let lastObservation;
  const controller = new AbortController();
  const timer = setInterval(() => void flush(), 4000);
  timer.unref();

  async function flush() {
    if (closed || signal.aborted || inFlight || !pending.size || calls >= 40 || Date.now() - lastSent < 4000) return;
    inFlight = true;
    calls++;
    lastSent = Date.now();
    const observedRevision = revision;
    const evidence = [...pending].join('\n');
    pending.clear();
    try {
      const timeout = AbortSignal.any([signal, controller.signal, AbortSignal.timeout(3000)]);
      // Enforce the deadline even if a translator ignores cancellation.
      const cancelled = new Promise((_, reject) => {
        timeout.addEventListener('abort', () => reject(timeout.reason), { once: true });
      });
      const result = await Promise.race([translate({
        request: prompt.slice(0, 2000), phase, observation: evidence,
        previousMessage: previous.join('\n').slice(-500),
      }, timeout), cancelled]);
      const message = businessText(result);
      if (!closed && !signal.aborted && revision === observedRevision && message && message !== 'RIEN'
        && !previous.some(text => text.toLocaleLowerCase() === message.toLocaleLowerCase())
        && message.length <= 240 && [...sentences.segment(message)].length <= 1
        && !/(?:^|[^\p{L}])(?:termin[ée]e?s?|disponibles?|publi[ée]e?s?|appliqu[ée]e?s?|prêt[es]*|réussi[es]*|enregistr[ée]e?s?)(?=$|[^\p{L}])/iu.test(message)) {
        previous.push(message);
        if (previous.length > 2) previous.shift();
        onProgress(phase, message);
      }
    } catch (error) {
      if ([401, 402, 403, 404].includes(error?.status)) calls = 40;
    } finally { inFlight = false; }
  }
  return {
    observe(fact) {
      if (closed || !phases.has(fact.phase)) return;
      const next = fact.phase;
      // A finite vocabulary bounds memory and prevents logs, paths or secrets entering the prompt.
      const status = ['pending', 'running', 'completed', 'error'].includes(fact.status) ? fact.status : 'observed';
      const area = ['screen', 'data', 'rules'].includes(fact.area) ? fact.area : 'application';
      const action = ['inspect', 'plan', 'edit', 'typecheck', 'test', 'build', 'dependencies', 'review'].includes(fact.action) ? fact.action : 'inspect';
      const evidence = JSON.stringify({ phase: next, status, area, action });
      // Tool identity stays private; only bounded categories reach the translator.
      const key = fact.id ? `${fact.id}:${status}` : evidence;
      if (fact.id ? observed.has(key) : lastObservation === key) return;
      if (phase !== next) { phase = next; revision++; pending.clear(); }
      lastObservation = key;
      if (fact.id) {
        observed.add(key);
        if (observed.size > 128) observed.delete(observed.values().next().value);
      }
      pending.add(evidence);
      void flush();
    },
    close() { closed = true; clearInterval(timer); controller.abort(); },
  };
}
