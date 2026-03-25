import { Button } from "@/components/ui/button";
import { layout } from "@/core/studio";

export default function SettingsPanel() {
  return (
    <div className="flex flex-col gap-4 p-3">
      <section>
        <h3 className="text-xs font-semibold uppercase tracking-wider text-primary mb-2">Quick links</h3>
        <div className="flex flex-col gap-1">
          <Button size="xs" variant="link" className="justify-start" onClick={() => layout.showView("llm-models")}>LLM Models</Button>
          <Button size="xs" variant="link" className="justify-start" onClick={() => layout.showView("secrets")}>Platform Secrets</Button>
          <Button size="xs" variant="link" className="justify-start" onClick={() => layout.showView("integrations")}>Integrations</Button>
        </div>
      </section>
    </div>
  );
}
