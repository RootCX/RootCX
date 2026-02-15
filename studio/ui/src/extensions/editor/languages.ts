import type { LanguageSupport } from "@codemirror/language";

const cache = new Map<string, LanguageSupport>();

const EXT_MAP: Record<string, { loader: () => Promise<LanguageSupport>; name: string }> = {
  js:   { loader: () => import("@codemirror/lang-javascript").then((m) => m.javascript()), name: "JavaScript" },
  jsx:  { loader: () => import("@codemirror/lang-javascript").then((m) => m.javascript({ jsx: true })), name: "JavaScript (JSX)" },
  ts:   { loader: () => import("@codemirror/lang-javascript").then((m) => m.javascript({ typescript: true })), name: "TypeScript" },
  tsx:  { loader: () => import("@codemirror/lang-javascript").then((m) => m.javascript({ jsx: true, typescript: true })), name: "TypeScript (TSX)" },
  html: { loader: () => import("@codemirror/lang-html").then((m) => m.html()), name: "HTML" },
  css:  { loader: () => import("@codemirror/lang-css").then((m) => m.css()), name: "CSS" },
  json: { loader: () => import("@codemirror/lang-json").then((m) => m.json()), name: "JSON" },
  md:   { loader: () => import("@codemirror/lang-markdown").then((m) => m.markdown()), name: "Markdown" },
  py:   { loader: () => import("@codemirror/lang-python").then((m) => m.python()), name: "Python" },
  rs:   { loader: () => import("@codemirror/lang-rust").then((m) => m.rust()), name: "Rust" },
  go:   { loader: () => import("@codemirror/lang-go").then((m) => m.go()), name: "Go" },
  java: { loader: () => import("@codemirror/lang-java").then((m) => m.java()), name: "Java" },
  c:    { loader: () => import("@codemirror/lang-cpp").then((m) => m.cpp()), name: "C" },
  cpp:  { loader: () => import("@codemirror/lang-cpp").then((m) => m.cpp()), name: "C++" },
  h:    { loader: () => import("@codemirror/lang-cpp").then((m) => m.cpp()), name: "C/C++ Header" },
  hpp:  { loader: () => import("@codemirror/lang-cpp").then((m) => m.cpp()), name: "C++ Header" },
  sql:  { loader: () => import("@codemirror/lang-sql").then((m) => m.sql()), name: "SQL" },
  xml:  { loader: () => import("@codemirror/lang-xml").then((m) => m.xml()), name: "XML" },
  yaml: { loader: () => import("@codemirror/lang-yaml").then((m) => m.yaml()), name: "YAML" },
  yml:  { loader: () => import("@codemirror/lang-yaml").then((m) => m.yaml()), name: "YAML" },
};

function getExt(filename: string): string {
  const dot = filename.lastIndexOf(".");
  return dot === -1 ? "" : filename.slice(dot + 1).toLowerCase();
}

export async function loadLanguage(filename: string): Promise<LanguageSupport | null> {
  const ext = getExt(filename);
  const entry = EXT_MAP[ext];
  if (!entry) return null;

  const cached = cache.get(ext);
  if (cached) return cached;

  const lang = await entry.loader();
  cache.set(ext, lang);
  return lang;
}

export function getLanguageName(filename: string): string {
  return EXT_MAP[getExt(filename)]?.name ?? "Plain Text";
}
