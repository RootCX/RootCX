import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, readFile, readdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = fileURLToPath(new URL("../", import.meta.url));
if (!process.argv[2]) {
  throw new Error("Usage: node scripts/check-ui-consumer.mjs <archive.tgz|--registry>");
}
const archive = process.argv[2] === "--registry" ? null : path.resolve(process.argv[2]);
const directory = await mkdtemp(path.join(tmpdir(), "rootcx-ui-consumer-"));
const run = (command, args, cwd) => execFileSync(command, args, { cwd, stdio: "inherit" });
const sdkRoot = path.join(repoRoot, "runtime/sdk");
const sdkManifest = JSON.parse(await readFile(path.join(sdkRoot, "package.json"), "utf8"));
let sdkArchive;

console.log(archive
  ? "Local UI and SDK archive verification; this does not verify npm availability."
  : "Registry verification; dependencies will be installed without overrides.");
console.log(`Checking UI in real scaffolded apps: ${directory}`);
if (archive) {
  run("pnpm", ["install", "--frozen-lockfile", "--ignore-scripts"], sdkRoot);
  run("pnpm", ["build"], sdkRoot);
  const [packed] = JSON.parse(execFileSync("npm",
    ["pack", "--ignore-scripts", "--json", "--pack-destination", directory],
    { cwd: sdkRoot, encoding: "utf8" }));
  sdkArchive = path.join(directory, packed.filename);
}
run("cargo", ["run", "--locked", "-p", "rootcx-scaffold", "--example", "ui-fixtures", "--", directory], repoRoot);
run("cargo", ["run", "--locked", "--manifest-path", path.join(repoRoot, "Cargo.toml"),
  "-p", "rootcx-cli", "--", "new", "cli"], directory);

for (const variant of ["simple", "auth", "agent", "cli"]) {
  const app = path.join(directory, variant);
  const manifestPath = path.join(app, "package.json");
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  assert.match(manifest.dependencies["@rootcx/ui"], /^\^0\.9\./,
    `${variant}: scaffold must require the new UI release before any local override`);
  assert.equal(manifest.dependencies["@rootcx/sdk"], `^${sdkManifest.version}`,
    `${variant}: scaffold must use the SDK release that removes desktop integration`);
  const globals = await readFile(path.join(app, "src/globals.css"), "utf8");
  assert.match(globals, /@import ["']@rootcx\/ui\/theme\.css["']/,
    `${variant}: missing package theme import`);
  assert.doesNotMatch(globals, /@theme|@custom-variant dark|:root\s*\{/,
    `${variant}: scaffold must not duplicate or override the package theme`);
  if (archive) {
    manifest.dependencies["@rootcx/ui"] = `file:${archive}`;
    manifest.dependencies["@rootcx/sdk"] = `file:${sdkArchive}`;
    await writeFile(manifestPath, JSON.stringify(manifest, null, 2) + "\n");
  }
  run("pnpm", ["install", "--ignore-scripts"], app);
  const sdkDist = path.join(app, "node_modules/@rootcx/sdk/dist");
  for (const name of await readdir(sdkDist, { recursive: true })) {
    if (!name.endsWith(".js")) continue;
    assert.doesNotMatch(await readFile(path.join(sdkDist, name), "utf8"), /__TAURI/,
      `${variant}: installed SDK still includes desktop integration in ${name}`);
  }
  run("node", ["--input-type=module", "-e", `
    import assert from "node:assert/strict";
    import * as ui from "@rootcx/ui";
    for (const name of ["Button", "Field", "SidebarProvider", "TooltipProvider"]) {
      assert.ok(name in ui, name + " must be exported");
    }
    for (const name of ["ThemeProvider", "useTheme", "AppShell", "DataTable",
      "FormDialog", "StatusBadge", "SidebarItem", "ChatScrollArea"]) {
      assert.ok(!(name in ui), name + " must not be exported");
    }
  `], app);
  run("pnpm", ["exec", "tsc", "--noEmit"], app);
  run("pnpm", ["build"], app);

  // The theme must discover classes inside node_modules, not only the app's JSX.
  const assets = path.join(app, "dist/assets");
  const cssFiles = (await readdir(assets)).filter((name) => name.endsWith(".css"));
  const css = (await Promise.all(cssFiles.map((name) => readFile(path.join(assets, name), "utf8")))).join("\n");
  assert.ok(css.includes(".glass-floating"), `${variant}: package component CSS was not generated`);
  assert.ok(css.includes("--control-height"), `${variant}: RootCX tokens were not included`);
  assert.ok(css.includes("@font-face"), `${variant}: Inter was not included`);
  console.log(`${variant}: TypeScript, production build and packaged theme passed.`);
}

console.log(`Consumer fixtures retained for inspection: ${directory}`);
