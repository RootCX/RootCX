import { Fragment, useSyncExternalStore } from "react";
import {
  ResizablePanelGroup,
  ResizablePanel,
  ResizableHandle,
} from "@/components/ui/resizable";
import { subscribe, getSnapshot, type EditorNode } from "./store";
import { EditorPane } from "./pane";

function RenderNode({ node, focusedPane }: { node: EditorNode; focusedPane: string }) {
  if (node.type === "pane") {
    return <EditorPane pane={node} isFocused={node.id === focusedPane} />;
  }

  return (
    <ResizablePanelGroup orientation={node.direction}>
      {node.children.map((child, i) => (
        <Fragment key={child.id}>
          {i > 0 && <ResizableHandle />}
          <ResizablePanel minSize="10%">
            <RenderNode node={child} focusedPane={focusedPane} />
          </ResizablePanel>
        </Fragment>
      ))}
    </ResizablePanelGroup>
  );
}

export default function EditorPanel() {
  const { root, focusedPane } = useSyncExternalStore(subscribe, getSnapshot);
  return (
    <div className="h-full">
      <RenderNode node={root} focusedPane={focusedPane} />
    </div>
  );
}
