export interface FuzzyMatch {
  score: number;
  indices: number[];
}

export function fuzzyMatch(query: string, text: string): FuzzyMatch | null {
  const q = query.toLowerCase();
  const t = text.toLowerCase();

  if (q.length === 0) return { score: 0, indices: [] };
  if (q.length > t.length) return null;

  const indices: number[] = [];
  let qi = 0;
  let score = 0;
  let prevIdx = -2;

  for (let ti = 0; ti < t.length && qi < q.length; ti++) {
    if (t[ti] === q[qi]) {
      indices.push(ti);

      // consecutive bonus
      if (ti === prevIdx + 1) score += 5;
      // word-boundary bonus
      if (ti === 0 || !isAlphaNum(t[ti - 1])) score += 10;
      // exact case bonus
      if (text[ti] === query[qi]) score += 1;

      score += 1; // base match score
      prevIdx = ti;
      qi++;
    }
  }

  if (qi < q.length) return null;
  return { score, indices };
}

function isAlphaNum(ch: string): boolean {
  const c = ch.charCodeAt(0);
  return (c >= 48 && c <= 57) || (c >= 65 && c <= 90) || (c >= 97 && c <= 122);
}
