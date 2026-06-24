import { spawn } from "node:child_process";
import { access, cp, mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const WORKSPACE = process.env.ROOTCX_WORKSPACE || "/workspace";
const SKILL_SOURCE = "/opt/rootcx/skills/rootcx";
const CORE_TOKEN_FILE = "/run/rootcx/core-token";
const ANTHROPIC_KEY_FILE = "/run/rootcx/anthropic-api-key";
const HOME = process.env.HOME || "/home/devbox";

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

async function exists(file) {
  try {
    await access(file);
    return true;
  } catch {
    return false;
  }
}

async function run(command, args, cwd) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd,
      env: process.env,
      stdio: "inherit",
    });
    child.on("error", reject);
    child.on("close", (code) => {
      if (code === 0) resolve();
      else reject(new Error(`${command} exited with code ${code}`));
    });
  });
}

async function prepareProject(projectDir, name) {
  if (!(await exists(projectDir))) {
    process.stdout.write("\x1b[36mPreparing your RootCX workspace…\x1b[0m\r\n");
    await run("rootcx", ["new", name], WORKSPACE);
  }

  await mkdir(path.join(projectDir, ".claude", "skills"), { recursive: true });
  await cp(SKILL_SOURCE, path.join(projectDir, ".claude", "skills", "rootcx"), {
    recursive: true,
    force: true,
  });

  await writeFile(
    path.join(projectDir, "CLAUDE.md"),
    `# RootCX hosted development sandbox

You are working interactively with the user on a production-quality RootCX application.

- Load and follow the rootcx skill before making architectural decisions.
- Work only inside this repository.
- Build a complete, polished, usable application rather than a placeholder.
- Use the generated RootCX scaffold and its existing dependencies.
- Keep manifest.json consistent with the frontend and backend.
- Validate changes with the available build or typecheck commands.
- Run rootcx deploy when the application is ready so the user can test it.
- Never create a Git commit unless the user explicitly asks for one.
- Never print, read, or expose credentials.
`,
  );

  if (!(await exists(path.join(projectDir, ".git")))) {
    await run("git", ["init", "-b", "main"], projectDir);
    await run("git", ["config", "user.name", "RootCX Builder"], projectDir);
    await run("git", ["config", "user.email", "builder@rootcx.local"], projectDir);
  }
}

async function main() {
  const sessionId = required("ROOTCX_SESSION_ID");
  const coreUrl = required("ROOTCX_CORE_URL");
  const [coreTokenRaw, anthropicKeyRaw] = await Promise.all([
    readFile(CORE_TOKEN_FILE, "utf8"),
    readFile(ANTHROPIC_KEY_FILE, "utf8"),
  ]);
  const coreToken = coreTokenRaw.trim();
  const anthropicApiKey = anthropicKeyRaw.trim();
  if (!coreToken) throw new Error("RootCX Core token is empty");
  if (!anthropicApiKey) throw new Error("Anthropic API key is empty");

  const name = appName(sessionId);
  const projectDir = path.join(WORKSPACE, name);
  await mkdir(WORKSPACE, { recursive: true });
  await mkdir(path.join(HOME, ".rootcx"), { recursive: true });
  await mkdir(path.join(HOME, ".claude"), { recursive: true });
  await writeFile(
    path.join(HOME, ".rootcx", "config.json"),
    JSON.stringify({ url: coreUrl, token: coreToken }, null, 2),
    { mode: 0o600 },
  );
  await prepareProject(projectDir, name);
  await writeFile(
    path.join(HOME, ".claude.json"),
    JSON.stringify({
      hasCompletedOnboarding: true,
      projects: {
        [projectDir]: { hasTrustDialogAccepted: true },
      },
    }, null, 2),
    { mode: 0o600 },
  );
  await writeFile(
    path.join(HOME, ".claude", "settings.json"),
    JSON.stringify({
      permissions: { defaultMode: "bypassPermissions" },
      skipDangerousModePermissionPrompt: true,
    }, null, 2),
    { mode: 0o600 },
  );

  process.env.ANTHROPIC_AUTH_TOKEN = anthropicApiKey;
  delete process.env.ANTHROPIC_API_KEY;
  process.env.TERM ||= "xterm-256color";

  const args = [
    "--dangerously-skip-permissions",
    ...(process.env.ROOTCX_CLAUDE_MODEL
      ? ["--model", process.env.ROOTCX_CLAUDE_MODEL]
      : []),
  ];

  const claude = spawn("claude", args, {
    cwd: projectDir,
    env: process.env,
    stdio: "inherit",
  });
  claude.on("error", (error) => {
    throw error;
  });
  const code = await new Promise((resolve) => claude.on("close", resolve));
  process.exitCode = typeof code === "number" ? code : 1;
}

main().catch((error) => {
  process.stderr.write(`\r\nRootCX sandbox error: ${error instanceof Error ? error.message : String(error)}\r\n`);
  process.exitCode = 1;
});
