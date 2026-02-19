import { DockLayout } from "./components/layout/dock-layout";
import { ScaffoldWizardPortal } from "./extensions/project/scaffold-wizard";

export default function App() {
  return (
    <>
      <DockLayout />
      <ScaffoldWizardPortal />
    </>
  );
}
