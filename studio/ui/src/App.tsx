import { DockLayout } from "./components/layout/dock-layout";
import { ScaffoldWizardPortal } from "./extensions/project/scaffold-wizard";
import { MigrationDialogPortal } from "./extensions/run/migration-dialog";
import { AISetupDialogPortal } from "./components/ai-setup-dialog";

export default function App() {
  return (
    <>
      <DockLayout />
      <ScaffoldWizardPortal />
      <MigrationDialogPortal />
      <AISetupDialogPortal />
    </>
  );
}
