import { readFileSync } from 'node:fs';

const instructions = readFileSync(new URL('./business-progress.md', import.meta.url), 'utf8');
const sentences = new Intl.Segmenter('fr', { granularity: 'sentence' });
// Presentation only: never awaited by the coding loop and never allowed to announce publication.
export function businessText(value) {
  if (typeof value !== 'string') return null;
  const text = value.trim();
  if (!text || text.length > 500 || /[`{}<>]|https?:|\b(?:rootcx|opencode|sql|postgres|typescript|javascript|npm|bun|vite|backend|frontend|migration|api|sdk|git|terminal|commit|deploy|déploiement|compil\w*)\b|(?:src|public|backend)\//i.test(text)) return null;
  return text;
}

export function narrator({ config, prompt, signal, onProgress }) {
  let phase = 'understanding';
  let notes = '';
  let inFlight = false;
  let lastSent = 0;
  let calls = 0;
  let closed = false;
  let previousMessage = '';
  const controller = new AbortController();
  const timer = setInterval(() => void flush(), 8000);
  timer.unref();

  async function flush() {
    if (closed || inFlight || !notes || calls >= 20 || Date.now() - lastSent < 8000) return;
    inFlight = true;
    calls++;
    lastSent = Date.now();
    const observedPhase = phase;
    const evidence = notes;
    notes = '';
    try {
      const response = await fetch(config.endpoint, {
        method: 'POST', redirect: 'error',
        headers: { 'content-type': 'application/json', 'x-api-key': config.apiKey, 'anthropic-version': '2023-06-01' },
        signal: AbortSignal.any([signal, controller.signal, AbortSignal.timeout(6000)]),
        body: JSON.stringify({ model: config.model, max_tokens: 160,
          system: instructions,
          messages: [{ role: 'user', content: JSON.stringify({ request: prompt.slice(0, 3000), phase: observedPhase, observation: evidence.slice(-4000), previousMessage }) }],
        }),
      });
      if (!response.ok) {
        if ([401, 402, 403].includes(response.status)) calls = 20;
        return;
      }
      const result = await response.json();
      if (result.stop_reason === 'max_tokens') return;
      const message = businessText(result.content?.filter(part => part.type === 'text').map(part => part.text).join(' '));
      if (!closed && phase === observedPhase && message && message !== 'RIEN' && message !== previousMessage
        && [...sentences.segment(message)].length <= 2
        && !/(?:^|[^\p{L}])(?:termin[ée]e?s?|disponibles?|publi[ée]e?s?|appliqu[ée]e?s?|prêt[es]*|réussi[es]*|enregistr[ée]e?s?)(?=$|[^\p{L}])/iu.test(message)) {
        previousMessage = message;
        onProgress(phase, message);
      }
    } catch { /* A slow narrator must never interrupt the actual work. */ }
    finally { inFlight = false; }
  }
  return {
    observe(event, nextPhase) {
      // Match the public phase: rereading a file does not restart the whole request.
      if (nextPhase && (nextPhase !== 'understanding' || ['understanding', 'waiting'].includes(phase))) phase = nextPhase;
      const part = event.properties?.part;
      if (part?.type === 'text' && part.time?.end && typeof part.text === 'string') notes = (notes + '\n' + part.text).slice(-4000);
      // Tool output and model reasoning never enter the presentation prompt.
      if (part?.type === 'tool' && nextPhase) {
        const status = ['pending', 'running', 'completed', 'error'].includes(part.state?.status) ? part.state.status : 'observed';
        notes = (notes + `\nObserved activity: ${nextPhase}; status: ${status}`).slice(-4000);
      }
      if (nextPhase && !notes) notes = `Observed activity: ${nextPhase}`;
      void flush();
    },
    close() { closed = true; clearInterval(timer); controller.abort(); },
  };
}
