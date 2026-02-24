export function requireEnv(name: string): string {
    const value = process.env[name];
    if (!value) {
        throw new Error(
            `Missing ${name}. Set it as a platform secret via the Runtime API: ` +
            `POST /api/v1/platform/secrets { "key": "${name}", "value": "..." }`,
        );
    }
    return value;
}
