import type { Disposable } from "./registry";

export class ExtensionContext {
  private disposables: Disposable[] = [];

  track<D extends Disposable>(d: D): D {
    this.disposables.push(d);
    return d;
  }

  dispose() {
    this.disposables.forEach((d) => d.dispose());
    this.disposables = [];
  }
}
