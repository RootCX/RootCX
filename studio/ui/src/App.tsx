import { DockLayout } from "./components/layout/dock-layout";
import { ScaffoldWizardPortal } from "./extensions/project/scaffold-wizard";
import { MigrationDialogPortal } from "./extensions/run/migration-dialog";
import { ProviderSetupPortal } from "./extensions/settings/provider-setup";

export default function App() {
  return (
    <>
      <DockLayout />
      <ScaffoldWizardPortal />
      <MigrationDialogPortal />
      <ProviderSetupPortal />
    </>
  );
}
