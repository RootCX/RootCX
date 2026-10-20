import { businessText, narrator, narrationGateway } from './narrator.mjs';
import { isDeepStrictEqual } from 'node:util';
import { collect, validateFiles } from './workspace.mjs';
import { build, isolated } from './sandbox.mjs';
import { coherenceGateway, planActive, settlePlan } from './plan.mjs';
import { closeEngine, engine, progressFor, narrationFact, providerError } from './opencode.mjs';

export async function code({ root, appId, prompt, narrationPrompt = prompt, conversationId, signal, config, onProgress = () => {}, compile = build, planTool = (args, signal) => isolated(root, args, { signal }), review = coherenceGateway(config) }) {
  const initial = await collect(root);
  // Survives repair turns: an approved plan is not reviewed again for this request.
  const planReview = {};
  onProgress('understanding');
  const runtime = await engine(root, config);
  const session = await runtime.conversation(conversationId);
  const streamController = new AbortController();
  const combined = AbortSignal.any([signal, streamController.signal]);
  const voice = narrator({ translate: narrationGateway(config), prompt: narrationPrompt, signal, onProgress });
  let skillLoaded = false;
  async function observeProgress() {
    for await (const event of runtime.subscribe(combined)) {
      const part = event.properties?.part;
      if ((part?.sessionID ?? event.properties?.sessionID) !== session) continue;
      if (part?.type === 'tool' && part.tool === 'skill' && part.state?.input?.name === 'rootcx' && part.state.status === 'completed') skillLoaded = true;
      const phase = progressFor(event);
      if (phase) onProgress(phase);
      if (phase) voice.observe(narrationFact(event, phase));
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
    let request = `The files in the current workspace are the current published sources; they supersede previous session edits. Application: ${appId}. User request: ${prompt}`;
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
      if (planActive(initial, current)) voice.observe({ phase: 'checking', status: 'running', action: 'review' });
      try {
        await settlePlan({ root, initial, collect, run: planTool, review, state: planReview, signal });
        const before = await collect(root);
        validateFiles(before);
        voice.observe({ phase: 'checking', status: 'running', action: 'build' });
        const frontend = await compile(root, appId, signal);
        const files = await collect(root);
        validateFiles(files);
        if (!isDeepStrictEqual(before, files)) throw new Error('Build modified source files; rebuild the final revision');
        console.info(JSON.stringify({ event: 'opencode.completed', appId, skillLoaded }));
        // Core emits this business summary only after successful publication.
        return { files, frontend, summary: businessText(answer) || 'Votre modification est disponible dans votre application.' };
      } catch (error) {
        if (signal.aborted || error.fatal) throw error;
        if (attempt === 2) { error.code = 'BUILD_FAILED'; throw error; }
        onProgress('repairing');
        voice.observe({ phase: 'repairing', status: 'running' });
        request = `The publication verification failed. Correct the real source files and run the checks again. Do not claim success until fixed. Then answer the owner in concise French about their business outcome. Diagnostic:\n${String(error.message).slice(-24000)}`;
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
