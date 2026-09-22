// @vitest-environment jsdom
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { RuntimeClient, type AuthMode } from "../client";
import { AuthGate, type AuthFormSlotProps } from "./AuthGate";
import { RuntimeProvider } from "./RuntimeProvider";

const user = { id: "director", email: "director@example.test", displayName: "Director", createdAt: "" };
const rootcx = { id: "rootcx", displayName: "RootCX" };
const other = { id: "company", displayName: "Company SSO" };
const baseMode: AuthMode = {
  authRequired: true,
  magicLinkEnabled: true, providers: [rootcx],
};
let root: Root;
let container: HTMLDivElement;
let form: AuthFormSlotProps | null;

async function mount(autoOidcLogin = true) {
  await act(async () => {
    root.render(
      <StrictMode>
        <RuntimeProvider>
          <AuthGate
            autoOidcLogin={autoOidcLogin}
            renderForm={props => {
              form = props;
              return <div data-form>{props.error}{props.providers.map(provider =>
                <button key={provider.id} onClick={() => props.onOidcLogin(provider.id)}>{provider.displayName}</button>,
              )}</div>;
            }}
          >
            {({ logout }) => <button onClick={() => void logout()}>Sign out</button>}
          </AuthGate>
        </RuntimeProvider>
      </StrictMode>,
    );
  });
}

async function remount() {
  await act(async () => root.unmount());
  root = createRoot(container);
  form = null;
  await mount();
}

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  sessionStorage.clear();
  localStorage.clear();
  window.history.replaceState({}, "", "/apps/erp/orders?view=pending#details");
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  form = null;
  vi.spyOn(RuntimeClient.prototype, "authMode").mockResolvedValue(baseMode);
  vi.spyOn(RuntimeClient.prototype, "me").mockRejectedValue(new Error("Unauthorized"));
  vi.spyOn(RuntimeClient.prototype, "oidcLogin").mockResolvedValue();
  vi.spyOn(RuntimeClient.prototype, "logout").mockResolvedValue();
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("automatic SSO", () => {
  it.each([
    ["RootCX SSO", baseMode, true, true],
    ["another SSO provider", { ...baseMode, providers: [other] }, true, true],
    ["multiple providers", { ...baseMode, providers: [rootcx, other] }, true, false],
    ["no configured provider", { ...baseMode, providers: [] }, true, false],
    ["explicit opt-out", baseMode, false, false],
  ])("%s", async (_name, mode, enabled, redirects) => {
    vi.mocked(RuntimeClient.prototype.authMode).mockResolvedValue(mode);
    await mount(enabled);
    expect(RuntimeClient.prototype.oidcLogin).toHaveBeenCalledTimes(redirects ? 1 : 0);
    if (redirects) expect(RuntimeClient.prototype.oidcLogin).toHaveBeenCalledWith(mode.providers[0].id);
    expect(container.querySelector("[data-form]") !== null).toBe(!redirects);
  });

  it("uses the explicitly selected provider when several are available", async () => {
    vi.mocked(RuntimeClient.prototype.authMode).mockResolvedValue({ ...baseMode, providers: [rootcx, other] });
    await mount();
    await act(async () => container.querySelectorAll("button")[1].click());
    expect(RuntimeClient.prototype.oidcLogin).toHaveBeenCalledExactlyOnceWith(other.id);
  });

  it("uses an existing session without redirecting", async () => {
    vi.mocked(RuntimeClient.prototype.me).mockResolvedValue(user);
    await mount();
    expect(container.textContent).toBe("Sign out");
    expect(RuntimeClient.prototype.oidcLogin).not.toHaveBeenCalled();
  });

  it("does not retry an interrupted navigation on remount, but permits manual retry", async () => {
    await mount();
    await remount();
    expect(RuntimeClient.prototype.oidcLogin).toHaveBeenCalledTimes(1);
    expect(form?.error).toContain("Sign-in did not complete");
    await act(async () => container.querySelector("button")!.click());
    expect(RuntimeClient.prototype.oidcLogin).toHaveBeenCalledTimes(2);
  });

  it("offers recovery when returning through the browser back-forward cache", async () => {
    await mount();
    await act(async () => window.dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true })));
    expect(form?.error).toContain("Sign-in did not complete");
    expect(RuntimeClient.prototype.oidcLogin).toHaveBeenCalledTimes(1);
  });

  it("shows a failed redirect without automatically retrying", async () => {
    vi.mocked(RuntimeClient.prototype.oidcLogin).mockRejectedValue(new Error("Network unavailable"));
    await mount();
    expect(form?.error).toBe("Unable to reach the server. Please check your connection.");
    expect(RuntimeClient.prototype.oidcLogin).toHaveBeenCalledTimes(1);
  });

  it("keeps sign-out effective across reloads", async () => {
    vi.mocked(RuntimeClient.prototype.me).mockResolvedValue(user);
    await mount();
    await act(async () => container.querySelector("button")!.click());
    expect(RuntimeClient.prototype.logout).toHaveBeenCalledTimes(1);
    expect(RuntimeClient.prototype.oidcLogin).not.toHaveBeenCalled();
    expect(container.querySelector("[data-form]")).not.toBeNull();
    vi.mocked(RuntimeClient.prototype.me).mockRejectedValue(new Error("Unauthorized"));
    await remount();
    expect(RuntimeClient.prototype.oidcLogin).not.toHaveBeenCalled();
    expect(form?.error).toBeNull();
  });

  it.each(["read", "write"] as const)("keeps manual sign-in available if session storage %s is blocked", async failure => {
    if (failure === "read") {
      vi.spyOn(window, "sessionStorage", "get").mockImplementation(() => { throw new Error("Blocked"); });
    } else {
      vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("Quota exceeded"); });
    }
    await mount();
    expect(container.querySelector("[data-form]")).not.toBeNull();
    expect(RuntimeClient.prototype.oidcLogin).not.toHaveBeenCalled();
    await act(async () => container.querySelector("button")!.click());
    expect(RuntimeClient.prototype.oidcLogin).toHaveBeenCalledExactlyOnceWith(rootcx.id);
  });

  it("waits for the single-use nonce exchange, including StrictMode effect replay", async () => {
    window.history.replaceState({}, "", "/apps/erp/orders?view=pending&auth_nonce=single-use#details");
    let finish!: (tokens: { accessToken: string; refreshToken: string }) => void;
    const exchange = vi.spyOn(RuntimeClient.prototype, "exchangeNonce").mockImplementation(() =>
      new Promise(resolve => { finish = resolve; }),
    );
    await mount();
    expect(exchange).toHaveBeenCalledTimes(1);
    expect(RuntimeClient.prototype.oidcLogin).not.toHaveBeenCalled();
    await act(async () => {
      vi.mocked(RuntimeClient.prototype.me).mockImplementation(async function (this: RuntimeClient) {
        if (this.getAccessToken() !== "access") throw new Error("Missing callback token");
        return user;
      });
      finish({ accessToken: "access", refreshToken: "refresh" });
    });
    expect(container.textContent).toBe("Sign out");
    expect(window.location.search + window.location.hash).toBe("?view=pending#details");
    expect(localStorage.getItem("rootcx_refresh_token")).toBe("refresh");
    expect(RuntimeClient.prototype.oidcLogin).not.toHaveBeenCalled();
  });

  it("offers manual recovery for an expired nonce even without a redirect marker", async () => {
    window.history.replaceState({}, "", "/apps/erp/?auth_nonce=expired");
    vi.spyOn(RuntimeClient.prototype, "exchangeNonce").mockRejectedValue(new Error("expired nonce"));
    await mount();
    expect(form?.error).toContain("Sign-in did not complete");
    expect(window.location.search).toBe("");
    expect(RuntimeClient.prototype.oidcLogin).not.toHaveBeenCalled();
  });

  it.each([
    ["?view=pending&access_token=access&refresh_token=refresh#access_token=access&refresh_token=refresh", "?view=pending"],
    ["?view=pending#access_token=access&refresh_token=refresh", "?view=pending"],
    ["?view=pending&access_token=access&refresh_token=refresh#details", "?view=pending#details"],
    ["?view=pending&access_token=incomplete#access_token=access&refresh_token=refresh", "?view=pending"],
  ])("consumes legacy callback tokens without leaving credentials in the URL: %s", async (input, expected) => {
    window.history.replaceState({}, "", `/apps/erp/orders${input}`);
    vi.mocked(RuntimeClient.prototype.me).mockResolvedValue(user);
    await mount();
    expect(window.location.search + window.location.hash).toBe(expected);
    expect(localStorage.getItem("rootcx_refresh_token")).toBe("refresh");
    expect(container.textContent).toBe("Sign out");
    expect(RuntimeClient.prototype.oidcLogin).not.toHaveBeenCalled();
  });
});
