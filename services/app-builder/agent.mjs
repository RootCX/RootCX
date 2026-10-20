import { conversationContext, contextText } from './narration-context.mjs';
import { businessText, narrator, narrationGateway } from './narrator.mjs';
import { isDeepStrictEqual } from 'node:util';
import { collect, validateFiles } from './workspace.mjs';
import { build, isolated } from './sandbox.mjs';
import { coherenceGateway, planActive, settlePlan } from './plan.mjs';
import { closeEngine, engine, progressFor, narrationFact, providerError } from './opencode.mjs';

export async function code({ root, appId, prompt, narrationPrompt = prompt, conversationId, runId, signal, config, onProgress = () => {}, compile = build, planTool = (args, signal) => isolated(root, args, { signal }), review = coherenceGateway(config) }) {
  const initial = await collect(root);
  // Survives repair turns: an approved plan is not reviewed again for this request.
  const planReview = {};
  onProgress('understanding');
  const runtime = await engine(root, config);
  const session = await runtime.conversation(conversationId);
  const streamController = new AbortController();
  const combined = AbortSignal.any([signal, streamController.signal]);
  const context = await conversationContext(root, session, runId, narrationPrompt);
  const voice = narrator({ context: context.previous, translate: narrationGateway(config), prompt: narrationPrompt, signal, onProgress });
  let skillLoaded = false;
  let intent = '';
  async function observeProgress() {
    for await (const event of runtime.subscribe(combined)) {
      const part = event.properties?.part;
      if ((part?.sessionID ?? event.properties?.sessionID) !== session) continue;
      if (part?.type === 'text' && part.time?.end) { intent = businessText(contextText(part.text, 240)) || ''; continue; }
      if (part?.type === 'tool' && part.tool === 'skill' && part.state?.input?.name === 'rootcx' && part.state.status === 'completed') skillLoaded = true;
      if (event.type === 'session.status') voice.working(event.properties.status?.type === 'busy');
      const phase = progressFor(event);
      if (phase) onProgress(phase);
      if (phase) {
        const fact = narrationFact(event, phase);
        // Only an outward statement followed by actual tool activity is an intention.
        // Final replies and private reasoning never become progress evidence.
        if (part?.type === 'tool' && part.state?.status === 'running' && intent) { fact.intent = intent; intent = ''; }
        voice.observe(fact);
      }
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
    let request = `The files in the current workspace are the current published sources; they supersede previous session edits. Application: ${appId}. User request: ${prompt}
Réponds au client en français courant, uniquement sur sa demande et le résultat métier. Aucun compte rendu de ton travail interne. Exemple après un changement de titre : « Le titre affiche désormais “Mes devis traiteur”. » Ne raconte ni les outils, ni les fichiers, ni les vérifications, ni les réparations.`;
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
        const summary = businessReply(answer);
        await context.reply(summary);
        return { files: current, frontend: '', summary };
      }
      // The native engine owns all coding/tool/reasoning loops. Publication verifies
      // the final revision and returns concrete failures to that same session.
      onProgress('checking');
      const reviewing = planActive(initial, current);
      if (reviewing) voice.observe({ phase: 'checking', status: 'running', action: 'review', source: 'platform' });
      try {
        await settlePlan({ root, initial, collect, run: planTool, review, state: planReview, signal });
        if (reviewing) voice.observe({ phase: 'checking', status: 'completed', action: 'review', source: 'platform' });
        const before = await collect(root);
        validateFiles(before);
        const frontend = await compile(root, appId, signal, fact => voice.observe({ ...fact, phase: 'checking', source: 'platform' }));
        const files = await collect(root);
        validateFiles(files);
        if (!isDeepStrictEqual(before, files)) throw new Error('Build modified source files; rebuild the final revision');
        console.info(JSON.stringify({ event: 'opencode.completed', appId, skillLoaded }));
        // Core emits this business summary only after successful publication.
        const summary = businessText(answer) || 'Votre modification est disponible dans votre application.';
        await context.reply(summary);
        return { files, frontend, summary };
      } catch (error) {
        if (signal.aborted || error.fatal) throw error;
        if (attempt === 2) { error.code = 'BUILD_FAILED'; throw error; }
        onProgress('repairing');
        voice.observe({ phase: 'repairing', status: 'error', source: 'platform', action: 'work' });
        request = `The publication verification failed. Correct the real source files and run the checks again. Do not claim success until fixed. Then answer the owner in concise French about their business outcome. Diagnostic:\n${String(error.message).slice(-24000)}
Le diagnostic ci-dessus est privé. Une fois le problème corrigé, réponds uniquement à la demande métier originale : ${narrationPrompt}. En une ou deux phrases françaises, décris le comportement obtenu, jamais le problème interne ou sa réparation. Exemple : « L’acompte est de 40 %, soit 400 € pour un devis de 1 000 €. » Ne mentionne ni construction, ni vérifications, ni fichiers, ni publication à venir.`;
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
