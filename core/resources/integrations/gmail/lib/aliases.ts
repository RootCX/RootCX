import { LruMap } from "./lru";

interface AliasEntry { primary: string; aliases: Set<string> }

const CACHE = new LruMap<string, AliasEntry>(128, 60 * 60_000);

export function cacheAliases(userId: string, primary: string, aliases: string[]): void {
  CACHE.set(userId, {
    primary: primary.toLowerCase(),
    aliases: new Set([primary.toLowerCase(), ...aliases.map(a => a.toLowerCase())]),
  });
}

export function getCachedAliases(userId: string): AliasEntry | null {
  return CACHE.get(userId) ?? null;
}

export function evictAliases(userId: string): void { CACHE.delete(userId); }
