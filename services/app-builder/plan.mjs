import { createHash } from 'node:crypto';

// The living plan: openspec/specs is the only current truth of an application's behaviour.
// Dated change folders under openspec/changes/archive are history; nothing reads them as truth.
const specs = /^openspec\/specs\/.+\/spec\.md$/;
// OpenSpec recommends English RFC 2119 keywords; plans are written in French for their owner.
// Informational notes are not defects; archive itself refuses a delta that cannot merge.
const tolerated = /should contain SHALL or MUST/;

function text(content) {
  return Buffer.from(content, 'base64').toString('utf8');
}

// Repairable errors go back to the coding session; fatal ones stop the publication.
function repair(message) {
  return Object.assign(new Error(message), { repairable: true });
}

function fatal(message, code) {
  return Object.assign(new Error(message), { fatal: true, code });
}

function reviewErrorCode(status) {
  if (status === 402) return 'AI_CREDITS_EXHAUSTED';
  if (status === 401 || status === 403) return 'AI_CONFIGURATION';
  return 'AI_UNAVAILABLE';
}

export function planActive(initial, current) {
  return [initial, current].some(files => Object.keys(files).some(path => specs.test(path)));
}

export function pendingChanges(files) {
  const names = new Set();
  for (const path of Object.keys(files)) {
    const match = /^openspec\/changes\/([^/]+)\//.exec(path);
    if (match && match[1] !== 'archive') names.add(match[1]);
  }
  return [...names].sort();
}

function issues(output) {
  let report;
  try { report = JSON.parse(output.slice(output.indexOf('{'))); } catch { return [output.trim().slice(-4000)]; }
  return (report.items ?? []).flatMap(item => (item.issues ?? [])
    .filter(issue => issue.level !== 'INFO' && !tolerated.test(issue.message))
    .map(issue => `${item.id}${issue.path ? ` (${issue.path})` : ''}: ${issue.message}`));
}

async function validate(run, args, signal) {
  let output;
  try { output = await run(['openspec', 'validate', ...args, '--strict', '--json', '--no-interactive'], signal); }
  catch (error) { if (error.fatal) throw error; output = error.message; }
  return issues(output);
}

// Merges every pending change into the plan with OpenSpec, then has the whole plan reviewed for coherence.
// The application's own tests run afterwards in the normal build.
export async function settlePlan({ root, initial, collect, run, review, state = {}, signal }) {
  let current = await collect(root);
  if (!planActive(initial, current)) return { files: current, active: false };

  for (const change of pendingChanges(current)) {
    const problems = await validate(run, [change, '--type', 'change'], signal);
    if (problems.length) throw repair(`The OpenSpec change "${change}" is invalid. Fix it, then finish again:\n${problems.join('\n')}`);
    try { await run(['openspec', 'archive', change, '--yes'], signal); }
    catch (error) {
      if (error.fatal) throw error;
      throw repair(`The OpenSpec change "${change}" cannot merge into the current specs. A requirement that already exists must be MODIFIED, not ADDED, and MODIFIED or REMOVED must use the exact current name:\n${error.message.slice(-4000)}`);
    }
  }
  current = await collect(root);

  const problems = await validate(run, ['--specs'], signal);
  if (problems.length) throw repair(`The OpenSpec specs are invalid:\n${problems.join('\n')}`);

  const plan = Object.keys(current).filter(path => specs.test(path)).sort().map(path => `<file path="${path}">\n${text(current[path])}\n</file>`).join('\n\n');
  const digest = createHash('sha256').update(plan).digest('hex');
  if (plan && state.reviewed !== digest) {
    const found = await review(plan, signal);
    if (found.length) {
      throw repair(`The OpenSpec specs are not coherent. Resolve every point with a new change that modifies or removes requirements, so the whole specs read as one consistent truth, update the matching tests, then finish again:\n${found.map(issue => `- ${issue.kind}: ${issue.requirements.join(' / ')}. ${issue.explanation}`).join('\n')}`);
    }
    state.reviewed = digest;
  }
  return { files: current, active: true };
}

const reviewInstructions = `You audit the living functional plan of a business application. The plan is the only current truth about how the application behaves; a developer implements it literally.
Report every problem that makes the plan contradictory or ambiguous:
- contradiction: two requirements, or a requirement and a scenario, that cannot both hold for the same situation (including across domains);
- duplicate: two requirements that govern the same behaviour, even with different names, wording or domains;
- inconsistent_example: a scenario whose WHEN/THEN does not follow from its requirement (check every calculation).
Report a contradiction only when you can name one concrete situation in which the two rules demand different outcomes. A rule that only restricts when another rule can apply is not a contradiction. Verify every calculation before reporting it, and never report something you verified as correct.
Do not report style, wording, missing features, or behaviour the plan does not describe. When the plan is coherent, report an empty list.
Always answer by calling report_coherence exactly once.`;

const reportTool = {
  name: 'report_coherence', strict: true,
  description: 'Report every coherence problem found in the plan.',
  input_schema: {
    type: 'object', additionalProperties: false, required: ['issues'],
    properties: { issues: { type: 'array', items: {
      type: 'object', additionalProperties: false, required: ['kind', 'requirements', 'explanation'],
      properties: {
        kind: { enum: ['contradiction', 'duplicate', 'inconsistent_example'] },
        requirements: { type: 'array', items: { type: 'string' }, minItems: 1 },
        explanation: { type: 'string' },
      },
    } } },
  },
};

// One model call without workspace access: the reviewer reads the whole plan and cannot change any file.
export function coherenceGateway({ endpoint, apiKey, model } = {}, transport = fetch) {
  return async (plan, signal) => {
    let response;
    try {
      response = await transport(endpoint, {
        method: 'POST', redirect: 'error', signal,
        headers: { 'content-type': 'application/json', 'x-api-key': apiKey, 'anthropic-version': '2023-06-01' },
        body: JSON.stringify({
          model, max_tokens: 16000, system: reviewInstructions,
          // Current Opus models reject forced tool choice and sampling parameters; a strict schema keeps the report valid.
          tools: [reportTool], tool_choice: { type: 'auto' }, output_config: { effort: 'high' },
          messages: [{ role: 'user', content: plan }],
        }),
      });
    } catch (error) {
      if (signal?.aborted) throw error;
      throw fatal('Plan review unavailable', 'AI_UNAVAILABLE');
    }
    if (!response.ok) throw fatal(`Plan review failed (${response.status})`, reviewErrorCode(response.status));
    const result = await response.json();
    const report = result.content?.find(part => part.type === 'tool_use' && part.name === reportTool.name)?.input;
    if (!Array.isArray(report?.issues)) throw fatal('Plan review returned no report', 'AI_UNAVAILABLE');
    return report.issues.filter(issue => typeof issue?.explanation === 'string' && Array.isArray(issue.requirements));
  };
}
