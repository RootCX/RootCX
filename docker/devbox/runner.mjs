import { spawn } from "node:child_process";
import { cp, mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import readline from "node:readline";
import { query } from "@anthropic-ai/claude-agent-sdk";

const WORKSPACE = process.env.ROOTCX_WORKSPACE || "/workspace";
const SKILL_SOURCE = "/opt/rootcx/skills/rootcx";
const PROMPT_FILE = "/run/rootcx/prompt.txt";
const CORE_TOKEN_FILE = "/run/rootcx/core-token";
const ANTHROPIC_KEY_FILE = "/run/rootcx/anthropic-api-key";
const HOME = process.env.HOME || "/home/devbox";

function emit(type, message, data = {}) {
  process.stdout.write(`${JSON.stringify({
    type,
    message,
    timestamp: new Date().toISOString(),
    ...data,
  })}\n`);
}

function required(name) {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function appName(sessionId) {
  const configured = process.env.ROOTCX_APP_NAME?.trim();
  if (configured) return configured.replace(/[^a-zA-Z0-9_]/g, "_").slice(0, 50);
  return `app_${sessionId.replace(/-/g, "").slice(0, 10)}`;
}

async function run(command, args, options = {}) {
  emit("tool", `${command} ${args.join(" ")}`, { tool: "Bash" });

  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: { ...process.env, ...options.env },
      stdio: ["ignore", "pipe", "pipe"],
    });

    const consume = (stream, level) => {
      const lines = readline.createInterface({ input: stream });
      lines.on("line", (line) => {
        if (line.trim()) emit("log", line, { stream: level });
      });
    };

    consume(child.stdout, "stdout");
    consume(child.stderr, "stderr");

    child.on("error", reject);
    child.on("close", (code) => {
      if (code === 0) resolve();
      else reject(new Error(`${command} exited with code ${code}`));
    });
  });
}

function summarizeTool(block) {
  const input = block.input || {};
  if (block.name === "Bash") return input.command || "Running command";
  if (block.name === "Read") return input.file_path || "Reading file";
  if (block.name === "Write") return input.file_path || "Writing file";
  if (block.name === "Edit") return input.file_path || "Editing file";
  if (block.name === "Glob") return input.pattern || "Searching files";
  if (block.name === "Grep") return input.pattern || "Searching code";
  if (block.name === "Skill") return input.skill || "Loading skill";
  return block.name;
}

function forwardSdkMessage(message) {
  if (message.type === "system" && message.subtype === "init") {
    emit("status", "Claude Code session started", {
      sessionId: message.session_id,
      model: message.model,
    });
    return;
  }

  if (message.type === "assistant") {
    for (const block of message.message?.content || []) {
      if (block.type === "text" && block.text?.trim()) {
        emit("assistant", block.text.trim());
      } else if (block.type === "tool_use") {
        emit("tool", summarizeTool(block), { tool: block.name });
      }
    }
    return;
  }

  if (message.type === "result") {
    if (message.subtype === "success") {
      emit("status", "Claude finished generating the application", {
        costUsd: message.total_cost_usd,
        turns: message.num_turns,
      });
    } else {
      emit("error", message.result || `Claude stopped: ${message.subtype}`);
    }
  }
}

async function main() {
  const sessionId = required("ROOTCX_SESSION_ID");
  const coreUrl = required("ROOTCX_CORE_URL");
  const publicCoreUrl = process.env.ROOTCX_PUBLIC_CORE_URL?.trim() || coreUrl;
  const [promptRaw, coreTokenRaw, anthropicKeyRaw] = await Promise.all([
    readFile(PROMPT_FILE, "utf8"),
    readFile(CORE_TOKEN_FILE, "utf8"),
    readFile(ANTHROPIC_KEY_FILE, "utf8"),
  ]);
  const prompt = promptRaw.trim();
  const coreToken = coreTokenRaw.trim();
  process.env.ANTHROPIC_API_KEY = anthropicKeyRaw.trim();

  if (!prompt) throw new Error("Prompt is empty");
  if (!coreToken) throw new Error("RootCX Core token is empty");
  if (!process.env.ANTHROPIC_API_KEY) throw new Error("Anthropic API key is empty");

  const name = appName(sessionId);
  const projectDir = path.join(WORKSPACE, name);

  emit("status", "Preparing isolated RootCX workspace", { appName: name });
  await mkdir(WORKSPACE, { recursive: true });
  await mkdir(path.join(HOME, ".rootcx"), { recursive: true });
  await writeFile(
    path.join(HOME, ".rootcx", "config.json"),
    JSON.stringify({ url: coreUrl, token: coreToken }, null, 2),
    { mode: 0o600 },
  );

  await run("rootcx", ["new", name], { cwd: WORKSPACE });
  await mkdir(path.join(projectDir, ".claude", "skills"), { recursive: true });
  await cp(SKILL_SOURCE, path.join(projectDir, ".claude", "skills", "rootcx"), {
    recursive: true,
  });

  await writeFile(
    path.join(projectDir, "CLAUDE.md"),
    `# RootCX hosted builder

You are building a production-quality RootCX application from a user's plain-language request.

- Use the rootcx skill before making architectural decisions.
- Work only inside this repository.
- Make sensible product decisions without asking follow-up questions.
- Build a complete, polished and usable application, not a placeholder.
- Use the generated RootCX scaffold and its existing dependencies.
- Keep manifest.json consistent with the frontend and backend.
- Validate the project with the available build or typecheck commands.
- Do not run rootcx deploy yourself; the host deploys after your work is complete.
- Never print, read, or expose credentials.
`,
  );

  await run("git", ["init", "-b", "main"], { cwd: projectDir });
  await run("git", ["config", "user.name", "RootCX Builder"], { cwd: projectDir });
  await run("git", ["config", "user.email", "builder@rootcx.local"], { cwd: projectDir });

  emit("status", "Building your application with Claude Code");

  for await (const message of query({
    prompt: `Build the RootCX application described below.\n\nUSER REQUEST:\n${prompt}`,
    options: {
      cwd: projectDir,
      settingSources: ["project"],
      skills: ["rootcx"],
      allowedTools: ["Read", "Write", "Edit", "Glob", "Grep", "Bash", "Skill"],
      permissionMode: "acceptEdits",
      maxTurns: Number.parseInt(process.env.ROOTCX_MAX_TURNS || "50", 10),
      maxBudgetUsd: Number.parseFloat(process.env.ROOTCX_MAX_BUDGET_USD || "15"),
      ...(process.env.ROOTCX_CLAUDE_MODEL
        ? { model: process.env.ROOTCX_CLAUDE_MODEL }
        : {}),
    },
  })) {
    forwardSdkMessage(message);
  }

  emit("status", "Deploying the generated application to RootCX");
  await run("rootcx", ["deploy"], { cwd: projectDir });

  const manifest = JSON.parse(await readFile(path.join(projectDir, "manifest.json"), "utf8"));
  const deployedAppId = manifest.appId || name;
  emit("complete", "Application built and deployed", {
    appId: deployedAppId,
    appName: name,
    appUrl: `${publicCoreUrl.replace(/\/$/, "")}/apps/${deployedAppId}/`,
    workspace: projectDir,
  });
}

main().catch((error) => {
  emit("error", error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
