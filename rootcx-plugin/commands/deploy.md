---
description: Deploy the current RootCX project to the connected Core — installs the manifest, uploads backend and frontend archives, and starts the worker.
---

The user wants to deploy the current project.

1. Verify `manifest.json` exists in the cwd. If not, tell the user to `cd` into the project directory.
2. If `package.json` exists and there is no `dist/` directory, run the build first (e.g. `bun run build` or `npm run build`). Ask the user which package manager to use if ambiguous.
3. Run: `rootcx deploy`

The CLI handles:
- `POST /api/v1/apps` with the manifest (install / update)
- Uploading `backend/` as tar.gz if present
- Uploading `dist/` as tar.gz if present
- Starting the worker if a backend was deployed

Report each step to the user. If any step fails, show the exact error from the CLI output.

After a successful deploy, offer to:
- List installed apps: `rootcx apps`
- Invoke the agent (for agent projects): `rootcx invoke <app_id> "<message>"`
