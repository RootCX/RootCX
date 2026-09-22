import { afterEach, expect, it, vi } from "vitest";
import { RuntimeClient } from "./client";

afterEach(() => vi.unstubAllGlobals());

it.each([
  ["?view=pending#details", "?view=pending#details"],
  ["?view=pending&auth_nonce=expired#details", "?view=pending#details"],
  ["?access_token=secret&refresh_token=secret&expires_in=900&view=pending#details", "?view=pending#details"],
  ["?view=pending#access_token=secret&refresh_token=secret", "?view=pending"],
])("preserves the destination and strips callback credentials: %s", async (input, expected) => {
  const location = { href: `https://tenant.example/apps/erp/orders${input}` };
  vi.stubGlobal("window", { location });
  await new RuntimeClient({ baseUrl: "https://tenant.example" }).oidcLogin("rootcx");
  const redirect = new URL(location.href);
  expect(redirect.pathname).toBe("/api/v1/auth/oidc/rootcx/authorize");
  expect(redirect.searchParams.get("token_delivery")).toBe("nonce");
  expect(redirect.searchParams.get("redirect_uri")).toBe(`https://tenant.example/apps/erp/orders${expected}`);
});
