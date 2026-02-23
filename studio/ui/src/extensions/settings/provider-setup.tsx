import { useState, useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";

interface Provider {
  id: string;
  label: string;
  envKey: string;
  placeholder: string;
}

const PROVIDERS: Provider[] = [
  { id: "anthropic", label: "Anthropic", envKey: "ANTHROPIC_API_KEY", placeholder: "sk-ant-..." },
  { id: "openai", label: "OpenAI", envKey: "OPENAI_API_KEY", placeholder: "sk-..." },
  { id: "bedrock", label: "Bedrock", envKey: "AWS_BEARER_TOKEN_BEDROCK", placeholder: "ABSK..." },
];

export function ProviderSetupPortal() {
  const [show, setShow] = useState(false);

  useEffect(() => {
    let cancelled = false;
    let attempt = 0;
    const check = () => {
      invoke<string[]>("list_platform_secrets")
        .then((keys) => {
          if (cancelled) return;
          if (!PROVIDERS.some((p) => keys.includes(p.envKey))) setShow(true);
        })
        .catch(() => {
          // Runtime may not be ready yet — retry a few times
          if (!cancelled && ++attempt < 5) setTimeout(check, 2000);
        });
    };
    // Delay initial check to let the runtime boot
    const timer = setTimeout(check, 3000);
    return () => { cancelled = true; clearTimeout(timer); };
  }, []);

  if (!show) return null;
  return createPortal(
    <ProviderSetup onDone={() => setShow(false)} />,
    document.body,
  );
}

function ProviderSetup({ onDone }: { onDone: () => void }) {
  const [step, setStep] = useState(0);
  const [provider, setProvider] = useState<Provider | null>(null);
  const [apiKey, setApiKey] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => { inputRef.current?.focus(); }, [step]);

  const handleSubmit = async () => {
    if (!provider || !apiKey.trim()) return;
    setSaving(true);
    setError(null);
    try {
      await invoke("set_platform_secret", { key: provider.envKey, value: apiKey.trim() });
      onDone();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setSaving(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-[20vh]" onClick={onDone}>
      <div className="absolute inset-0 bg-black/50" />
      <div
        className="relative w-full max-w-md min-h-[120px] rounded-lg border border-border bg-card shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.key === "Escape" && onDone()}
      >
        {/* Progress */}
        <div className="h-px bg-border">
          <div
            className="h-px bg-primary/60 transition-all duration-300"
            style={{ width: `${((step + 1) / 2) * 100}%` }}
          />
        </div>

        {step === 0 && (
          <div className="px-3 py-2">
            <div className="flex items-center text-xs text-muted-foreground mb-2">
              <span>Provider Setup</span>
              <span className="ml-auto opacity-40">1/2</span>
            </div>
            <p className="text-[10px] text-muted-foreground mb-3">
              Choose your AI provider to get started.
            </p>
            <div className="flex gap-3">
              {PROVIDERS.map((p) => (
                <Button
                  key={p.id}
                  variant="outline"
                  onClick={() => { setProvider(p); setStep(1); }}
                >
                  {p.label}
                </Button>
              ))}
            </div>
            <div className="mt-2 text-[10px] text-muted-foreground/40">
              You can add more keys later in Settings.
            </div>
          </div>
        )}

        {step === 1 && provider && (
          <div className="px-3 py-2">
            <div className="flex items-center text-xs text-muted-foreground mb-2">
              <span>{provider.label} API Key</span>
              <span className="ml-auto opacity-40">2/2</span>
            </div>
            <input
              ref={inputRef}
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleSubmit()}
              placeholder={provider.placeholder}
              className="w-full bg-transparent text-sm text-foreground placeholder:text-muted-foreground outline-none"
            />
            <div className="flex items-center gap-2 mt-2">
              <Button size="sm" className="h-7 text-[10px]" onClick={handleSubmit} disabled={!apiKey.trim() || saving}>
                {saving ? "Saving..." : "Save"}
              </Button>
              <button
                className="text-[10px] text-muted-foreground hover:text-foreground"
                onClick={() => { setStep(0); setApiKey(""); setError(null); }}
              >
                Back
              </button>
            </div>
            {error && (
              <div className="mt-2 rounded-md border border-red-800 bg-red-950 px-2 py-1 text-[10px] text-red-300">
                {error}
              </div>
            )}
            <div className="mt-2 text-[10px] text-muted-foreground/40">
              Stored as {provider.envKey} in platform secrets.
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
