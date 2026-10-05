import { businessText, narrator } from './narrator.mjs';
import { isDeepStrictEqual } from 'node:util';
import { collect, validateFiles } from './workspace.mjs';
import { build } from './sandbox.mjs';
import { closeEngine, engine, events, progressFor, providerError } from './opencode.mjs';

export async function code({ root, appId, prompt, conversationId, signal, config, onProgress = () => {}, compile = build }) {
  const initial = await collect(root);
  onProgress('understanding');
  const runtime = await engine(root, config);
  const session = await runtime.conversation(conversationId);
  const streamController = new AbortController();
  const combined = AbortSignal.any([signal, streamController.signal]);
  const response = await fetch(`${runtime.url}/event`, { headers: runtime.headers, signal: combined });
  if (!response.ok) throw new Error('OpenCode events unavailable');
  const voice = narrator({ config, prompt, signal, onProgress });
  let skillLoaded = false;
  async function observeProgress() {
    for await (const event of events(response.body)) {
      const part = event.properties?.part;
      if ((part?.sessionID ?? event.properties?.sessionID) !== session) continue;
      if (part?.type === 'tool' && part.tool === 'skill' && part.state?.input?.name === 'rootcx' && part.state.status === 'completed') skillLoaded = true;
      const phase = progressFor(event);
      if (phase) onProgress(phase);
      voice.observe(event, phase);
    }
    throw new Error('OpenCode event stream disconnected');
  }
  const observing = observeProgress();
  // A disconnected event stream must not become an unhandled rejection.
  observing.catch(() => {});
  let aborting;
  function abort() {
    // Finish cancellation before the workspace/session can accept the next request.
    aborting ??= runtime.api(`/session/${session}/abort`, {}, AbortSignal.timeout(3000)).catch(() => closeEngine(root));
    return aborting;
  }
  signal.addEventListener('abort', abort, { once: true });
  try {
    let request = `Use the rootcx skill for this request. The files in /workspace are the current published sources; they supersede previous session edits. Application: ${appId}. User request: ${prompt}`;
    for (let attempt = 0; attempt < 3; attempt++) {
      const result = await Promise.race([
        runtime.api(`/session/${session}/message`, {
          agent: 'build',
          model: { providerID: 'anthropic', modelID: config.model },
          parts: [{ type: 'text', text: request }],
        }, signal),
        observing,
      ]);
      if (result.info?.error) {
        const error = new Error(`OpenCode: ${JSON.stringify(result.info.error)}`);
        error.code = providerError(result.info.error);
        throw error;
      }
      if (result.info?.finish === 'length') throw Object.assign(new Error('OpenCode output limit reached'), { code: 'AI_LIMIT_REACHED' });
      const current = await collect(root);
      const answer = (result.parts ?? []).filter(part => part.type === 'text').map(part => part.text).join('\n');
      if (isDeepStrictEqual(initial, current)) {
        return { files: current, frontend: '', summary: businessReply(answer) };
      }
      // The native engine owns all coding/tool/reasoning loops. Publication verifies
      // the final revision and returns concrete failures to that same session.
      onProgress('checking');
      voice.observe({}, 'checking');
      try {
        const before = await collect(root);
        validateFiles(before);
        const frontend = await compile(root, appId, signal);
        const files = await collect(root);
        validateFiles(files);
        if (!isDeepStrictEqual(before, files)) throw new Error('Build modified source files; rebuild the final revision');
        console.info(JSON.stringify({ event: 'opencode.completed', appId, skillLoaded }));
        // Core emits this business summary only after successful publication.
        return { files, frontend, summary: businessText(answer) || 'Votre modification est disponible dans votre application.' };
      } catch (error) {
        if (signal.aborted) throw error;
        if (attempt === 2) { error.code = 'BUILD_FAILED'; throw error; }
        onProgress('repairing');
        voice.observe({}, 'repairing');
        request = `The publication verification failed. Correct the real source files and run the checks again. Do not claim success until fixed. Diagnostic:\n${String(error.message).slice(-24000)}`;
      }
    }
  } finally {
    voice.close();
    await abort();
    signal.removeEventListener('abort', abort);
    streamController.abort();
    await observing.catch(() => {});
  }
}

export function businessReply(text) {
  const clean = text.replace(/```[\s\S]*?```/g, '').trim();
  if (!clean || /`|https?:|\b(?:rootcx|opencode|sql|postgres|typescript|javascript|npm|bun|vite|backend|frontend|migration|api|sdk|git|terminal)\b|(?:src|public|backend)\//i.test(clean)) {
    return 'Je suis là pour vous aider à faire évoluer votre application. Dites-moi ce que vous souhaitez changer.';
  }
  return clean.slice(0, 2000);
}
