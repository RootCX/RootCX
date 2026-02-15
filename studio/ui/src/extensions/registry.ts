type Listener = () => void;

export interface Disposable {
  dispose(): void;
}

export type Entry<T> = T & { id: string };

export class Registry<T> {
  private items = new Map<string, T>();
  private listeners = new Set<Listener>();
  private snapshot: Entry<T>[] | null = null;

  register(id: string, item: T): Disposable {
    this.items.set(id, item);
    this.snapshot = null;
    this.notify();
    return {
      dispose: () => {
        this.items.delete(id);
        this.snapshot = null;
        this.notify();
      },
    };
  }

  get(id: string): T | undefined {
    return this.items.get(id);
  }

  getAll(): Entry<T>[] {
    return (this.snapshot ??= [...this.items.entries()].map(([id, item]) => ({ ...item, id })));
  }

  subscribe(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private notify() {
    this.listeners.forEach((fn) => fn());
  }
}
