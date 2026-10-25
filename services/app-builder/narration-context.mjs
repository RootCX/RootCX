import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { createHash, randomUUID } from 'node:crypto';

// This is conversation context, never source code, tool output or proof of publication.
export function contextText(value, limit = 800) {
  if (typeof value !== 'string') return '';
  return value.replace(/```[\s\S]*?```/g, '[contenu technique retiré]')
    .replace(/https?:\/\/\S+|\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b/gi, '[coordonnées retirées]')
    .replace(/\b(?:bearer\s+\S+|(?:sk|rk|AKIA)[-_A-Za-z0-9]{12,})\b/gi, '[secret retiré]')
    .replace(/(?:password|mot de passe|api[_ -]?key|token|secret)\s*[:=]\s*\S+/gi, '[secret retiré]')
    .replace(/[\u0000-\u001f]+/g, ' ').trim().slice(0, limit);
}

export async function conversationContext(root, session, runId = randomUUID(), request = '') {
  // A separate sibling is not mounted in the coding sandbox; the model cannot rewrite this history.
  const directory = `${root}.narration`;
  const file = join(directory, createHash('sha256').update(session).digest('hex') + '.json');
  let turns = [];
  try {
    const saved = JSON.parse(await readFile(file, 'utf8'));
    if (Array.isArray(saved)) turns = saved.filter(turn => typeof turn.id === 'string' && typeof turn.request === 'string').slice(-4);
  } catch { /* Optional context must never prevent coding. */ }
  const previous = turns.filter(turn => turn.id !== runId).slice(-3).map(turn => ({
    user: contextText(turn.request), assistantPrepared: contextText(turn.reply), publication: 'unknown',
  }));
  const turn = { id: runId, request: contextText(request), reply: '' };
  turns = [...turns.filter(item => item.id !== runId), turn].slice(-4);
  async function save() {
    try {
      await mkdir(directory, { recursive: true, mode: 0o700 });
      await writeFile(file + '.tmp', JSON.stringify(turns), { mode: 0o600 });
      await rename(file + '.tmp', file);
    } catch { /* Narration history is best effort, never execution authority. */ }
  }
  await save();
  return { previous, async reply(text) { turn.reply = contextText(text); await save(); } };
}
