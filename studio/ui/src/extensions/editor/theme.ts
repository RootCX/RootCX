import { EditorView } from "@codemirror/view";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags } from "@lezer/highlight";

const bg = "#0d0d0d";
const activeLine = "#1e1e2e";
const border = "#262626";
const cursor = "#3b82f6";
const selection = "rgba(59, 130, 246, 0.25)";
const fg = "#fafafa";
const muted = "#a1a1aa";

export const studioTheme = EditorView.theme(
  {
    "&": {
      color: fg,
      backgroundColor: bg,
      fontSize: "13px",
      fontFamily: '"JetBrains Mono", "Fira Code", ui-monospace, monospace',
    },
    ".cm-content": { caretColor: cursor, padding: "4px 0" },
    ".cm-cursor, .cm-dropCursor": { borderLeftColor: cursor },
    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
      backgroundColor: selection,
    },
    ".cm-activeLine": { backgroundColor: activeLine },
    ".cm-gutters": {
      backgroundColor: bg,
      color: muted,
      border: "none",
      borderRight: `1px solid ${border}`,
    },
    ".cm-activeLineGutter": { backgroundColor: activeLine, color: fg },
    ".cm-foldPlaceholder": { backgroundColor: "transparent", border: "none", color: muted },
    ".cm-tooltip": { backgroundColor: "#141414", border: `1px solid ${border}`, color: fg },
    ".cm-tooltip-autocomplete": {
      "& > ul > li[aria-selected]": { backgroundColor: activeLine },
    },
    ".cm-panels": { backgroundColor: "#141414", color: fg },
    ".cm-panels.cm-panels-top": { borderBottom: `1px solid ${border}` },
    ".cm-panels.cm-panels-bottom": { borderTop: `1px solid ${border}` },
    ".cm-searchMatch": { backgroundColor: "rgba(59, 130, 246, 0.3)" },
    ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "rgba(59, 130, 246, 0.5)" },
  },
  { dark: true },
);

export const studioHighlighting = syntaxHighlighting(
  HighlightStyle.define([
    { tag: [tags.keyword, tags.controlKeyword, tags.operatorKeyword, tags.definitionKeyword, tags.moduleKeyword], color: "#cba6f7" },
    { tag: tags.operator, color: "#f38ba8" },
    { tag: [tags.string, tags.special(tags.string)], color: "#a6e3a1" },
    { tag: tags.number, color: "#f9e2af" },
    { tag: [tags.bool, tags.null], color: "#fab387" },
    { tag: [tags.comment, tags.blockComment, tags.lineComment], color: "#6c7086", fontStyle: "italic" },
    { tag: [tags.typeName, tags.className], color: "#89dceb" },
    { tag: [tags.function(tags.variableName), tags.function(tags.propertyName)], color: "#89b4fa" },
    { tag: [tags.definition(tags.variableName), tags.variableName], color: "#cdd6f4" },
    { tag: tags.propertyName, color: "#b4befe" },
    { tag: [tags.self, tags.tagName], color: "#f38ba8" },
    { tag: [tags.regexp, tags.escape], color: "#f5c2e7" },
    { tag: [tags.attributeName, tags.meta], color: "#f9e2af" },
    { tag: tags.attributeValue, color: "#a6e3a1" },
    { tag: tags.heading, color: "#f38ba8", fontWeight: "bold" },
    { tag: tags.link, color: "#89b4fa", textDecoration: "underline" },
    { tag: tags.emphasis, fontStyle: "italic" },
    { tag: tags.strong, fontWeight: "bold" },
    { tag: tags.processingInstruction, color: "#cba6f7" },
    { tag: [tags.punctuation, tags.bracket], color: "#bac2de" },
  ]),
);
