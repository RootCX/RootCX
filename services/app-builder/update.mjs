import { execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { isDeepStrictEqual, promisify } from 'node:util';
import { code } from './agent.mjs';
import { build } from './sandbox.mjs';
import { collect, materialize, replace, validateFiles } from './workspace.mjs';

const exec = promisify(execFile);
const version = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;
const markers = /^(?:<{7}|>{7})(?: |$)/m;
// Git keeps both sides of a file/directory clash under these suffixes.
const sideCopy = /~(?:HEAD|release)$/;

// Unmerged index stages: 1 base, 2 company, 3 release. Not every conflict leaves markers.
export function describeConflicts(listing) {
  const stages = new Map();
  for (const entry of listing.split('\0').filter(Boolean)) {
    const [meta, path] = entry.split('\t');
    if (!stages.has(path)) stages.set(path, new Set());
    stages.get(path).add(meta.split(' ')[2]);
  }
  return [...stages].map(([path, found]) => ({ path, kind: conflictKind(found) }));
}

function conflictKind(stages) {
  const [base, company, release] = ['1', '2', '3'].map(stage => stages.has(stage));
  if (company && release) return 'edited on both sides (conflict markers)';
  if (company) return base ? 'deleted by the new version, edited by the company' : 'added or renamed by the company only';
  if (release) return base ? 'deleted by the company, edited by the new version' : 'added or renamed by the new version only';
  return 'renamed differently on both sides';
}

export async function fetchPlan({ updatesEndpoint, apiKey }, { appId, conversationId, signal }) {
  if (!updatesEndpoint || !conversationId) return null;
  const url = new URL(updatesEndpoint);
  url.search = new URLSearchParams({ appId, conversationId });
  const response = await fetch(url, { redirect: 'error', signal, headers: { 'x-api-key': apiKey } });
  if (response.status === 204) return null;
  // An update conversation never falls back to an unrelated AI change.
  if (!response.ok) throw new Error(`Update plan unavailable (${response.status})`);
  const plan = await response.json();
  if (!['merge', 'reconcile'].includes(plan?.mode) || ![plan.base, plan.release].every(side => version.test(side?.version))) throw new Error('Invalid update plan');
  validateFiles(plan.base.files);
  validateFiles(plan.release.files);
  return plan;
}

export async function reportUpdate({ updatesEndpoint, apiKey }, outcome) {
  try {
    const response = await fetch(updatesEndpoint, {
      method: 'POST', redirect: 'error', signal: AbortSignal.timeout(10_000),
      headers: { 'content-type': 'application/json', 'x-api-key': apiKey },
      body: JSON.stringify(outcome),
    });
    if (!response.ok) throw new Error(`status ${response.status}`);
  } catch (error) { console.error('Update report failed:', error.message); }
}

// Real three-way Git merge, isolated from host configuration, hooks and attributes.
export async function mergeRelease({ base, ours, theirs }) {
  const dir = await mkdtemp(join(tmpdir(), 'rootcx-merge-'));
  const identity = { name: 'SHAPP', email: 'builder@shapp.ai' };
  const env = {
    PATH: process.env.PATH, HOME: dir, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null',
    GIT_AUTHOR_NAME: identity.name, GIT_AUTHOR_EMAIL: identity.email, GIT_COMMITTER_NAME: identity.name, GIT_COMMITTER_EMAIL: identity.email,
  };
  const git = async (...args) => (await exec('git', ['-c', 'core.hooksPath=/dev/null', '-c', 'commit.gpgSign=false', '-c', 'core.autocrlf=false', ...args], { cwd: dir, env, maxBuffer: 16 * 1024 * 1024 })).stdout;
  const commit = async (files, message) => {
    await git('rm', '-rqf', '--ignore-unmatch', '--', '.');
    await materialize(dir, files);
    await git('add', '-A');
    await git('commit', '-q', '--allow-empty', '-m', message);
  };
  try {
    await git('init', '-q', '-b', 'base');
    await commit(base, 'base');
    await git('checkout', '-q', '-b', 'release');
    await commit(theirs, 'release');
    await git('checkout', '-q', '-b', 'company', 'base');
    await commit(ours, 'company');
    try {
      await git('merge', '--no-ff', '--no-edit', '-q', 'release');
      return { clean: true, files: await collect(dir) };
    } catch (error) {
      const details = describeConflicts(await git('ls-files', '-u', '-z'));
      if (!details.length) throw error;
      const files = await collect(dir);
      const clash = 'copy kept by Git because a file and a folder clash; merge it into the right path and delete it';
      for (const path of Object.keys(files).filter(path => sideCopy.test(path))) {
        const known = details.find(item => item.path === path);
        if (known) known.kind = clash; else details.push({ path, kind: clash });
      }
      return { clean: false, files, details, conflicts: [...new Set(details.map(({ path }) => path.replace(sideCopy, '')))] };
    }
  } finally { await rm(dir, { recursive: true, force: true }); }
}

export async function applyUpdate({ plan, root, appId, conversationId, runId, prompt, narrationPrompt, signal, config, onProgress = () => {}, coding = code, compile = build, report = reportUpdate }) {
  const ours = await collect(root);
  const { base, release } = plan;
  const merged = await mergeRelease({ base: base.files, ours, theirs: release.files });
  if (plan.mode === 'merge' && !merged.clean) {
    await report(config, { appId, conversationId, outcome: 'conflict', conflicts: merged.conflicts });
    // Returning the input files publishes nothing: Core records an answer, not a revision.
    return { files: ours, frontend: '', summary: `La version ${release.version} modifie aussi des éléments que vous aviez personnalisés. Rien n’a été changé dans votre application. Dans « Mises à jour », vous pouvez laisser Shappy concilier les deux ou garder votre version.` };
  }
  await replace(root, merged.files);
  try {
    if (plan.mode === 'merge') {
      onProgress('checking');
      const frontend = await compile(root, appId, signal);
      return { files: merged.files, frontend, summary: `Votre application est à jour (version ${release.version}). Vos modifications sont conservées.` };
    }
    const conflicted = merged.clean ? 'The merge has no conflict but did not pass verification; fix it.'
      : `Unresolved merge items, each must be resolved: ${merged.details.map(({ path, kind }) => `${path} (${kind})`).join('; ')}.`;
    const request = `This request updates the application from version ${base.version} to version ${release.version}, published by SHAPP. The workspace holds the Git merge of the company's published revision with the new version. ${conflicted} Keep both the company's customizations and the new version's behavior. Do not change anything else. Owner request: ${prompt}`;
    const result = await coding({ root, appId, prompt: request, narrationPrompt: narrationPrompt ?? prompt, conversationId, runId, signal, config, onProgress });
    if (Object.keys(result.files).some(path => sideCopy.test(path)) || Object.values(result.files).some(content => markers.test(Buffer.from(content, 'base64').toString('latin1')))) throw new Error('Unresolved merge items remain after reconciliation');
    // The coder skips verification when it leaves the merged tree untouched.
    if (!result.frontend && !isDeepStrictEqual(result.files, ours)) return { ...result, frontend: await compile(root, appId, signal) };
    return result;
  } catch (error) {
    if (!signal.aborted) await report(config, { appId, conversationId, outcome: 'failed' });
    error.code ??= 'BUILD_FAILED';
    throw error;
  }
}
