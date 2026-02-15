import { useStatusBarItems } from "@/extensions/hooks";

export function StatusBar() {
  const items = useStatusBarItems();
  const left = items.filter((i) => i.alignment === "left");
  const right = items.filter((i) => i.alignment === "right");

  return (
    <div className="flex h-6 shrink-0 items-center border-t border-border bg-sidebar px-2">
      <div className="flex items-center">
        {left.map((item) => (
          <item.component key={item.id} />
        ))}
      </div>
      <div className="ml-auto flex items-center">
        {right.map((item) => (
          <item.component key={item.id} />
        ))}
      </div>
    </div>
  );
}
