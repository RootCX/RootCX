interface Entry<V> { value: V; touched: number }

export class LruMap<K, V> {
  private map = new Map<K, Entry<V>>();
  private readonly max: number;
  private readonly ttlMs: number;

  constructor(max: number, ttlMs: number) {
    this.max = max;
    this.ttlMs = ttlMs;
  }

  get(key: K): V | undefined {
    const e = this.map.get(key);
    if (!e) return undefined;
    if (Date.now() - e.touched > this.ttlMs) {
      this.map.delete(key);
      return undefined;
    }
    e.touched = Date.now();
    this.map.delete(key);
    this.map.set(key, e);
    return e.value;
  }

  set(key: K, value: V): void {
    if (this.map.size >= this.max && !this.map.has(key)) {
      const oldest = this.map.keys().next().value;
      if (oldest !== undefined) this.map.delete(oldest);
    }
    this.map.delete(key);
    this.map.set(key, { value, touched: Date.now() });
  }

  delete(key: K): void { this.map.delete(key); }
  get size(): number { return this.map.size; }
}
