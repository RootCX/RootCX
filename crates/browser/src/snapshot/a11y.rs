use std::collections::HashMap;
use std::fmt::Write;

use chromiumoxide::cdp::browser_protocol::accessibility::{AxNode, AxPropertyName, GetFullAxTreeParams};
use chromiumoxide::Page;

use crate::error::BrowserError;
use super::refs::RefRegistry;

pub struct ExtractConfig {
    pub max_nodes: usize,
    pub max_chars: usize,
    pub max_depth: usize,
    pub compact: bool,
    pub interactive_only: bool,
}

impl ExtractConfig {
    pub fn full() -> Self {
        Self { max_nodes: 500, max_chars: 30_000, max_depth: 10, compact: true, interactive_only: false }
    }
    pub fn efficient() -> Self {
        Self { max_nodes: 300, max_chars: 8_000, max_depth: 6, compact: true, interactive_only: true }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Role { Interactive, Content, Text, Structural, Skip }

fn classify(role: &str) -> Role {
    match role {
        "link" | "button" | "textbox" | "searchbox" | "combobox" | "listbox"
        | "checkbox" | "radio" | "switch" | "slider" | "spinbutton" | "tab"
        | "menuitem" | "menuitemcheckbox" | "menuitemradio" | "option"
        | "treeitem" | "textarea" | "DisclosureTriangle" => Role::Interactive,

        "heading" | "article" | "cell" | "gridcell" | "columnheader" | "rowheader"
        | "listitem" | "status" | "alert" | "dialog" | "alertdialog"
        | "blockquote" | "caption" | "figure" | "img" | "math"
        | "tooltip" | "log" | "marquee" | "timer" => Role::Content,

        "StaticText" | "paragraph" => Role::Text,

        "none" | "presentation" | "generic" | "LineBreak" | "InlineTextBox" => Role::Skip,

        _ => Role::Structural,
    }
}

fn ax_str(v: &Option<chromiumoxide::cdp::browser_protocol::accessibility::AxValue>) -> &str {
    v.as_ref().and_then(|v| v.value.as_ref()).and_then(|v| v.as_str()).unwrap_or("")
}

fn prop_bool(node: &AxNode, prop: AxPropertyName) -> bool {
    node.properties.as_ref().map_or(false, |ps| {
        ps.iter().any(|p| p.name == prop && p.value.value.as_ref().and_then(|v| v.as_bool()).unwrap_or(false))
    })
}

fn prop_str<'a>(node: &'a AxNode, prop: AxPropertyName) -> Option<&'a str> {
    node.properties.as_ref().and_then(|ps| {
        ps.iter().find(|p| p.name == prop).and_then(|p| p.value.value.as_ref().and_then(|v| v.as_str()))
    })
}

fn prop_i64(node: &AxNode, prop: AxPropertyName) -> Option<i64> {
    node.properties.as_ref().and_then(|ps| {
        ps.iter().find(|p| p.name == prop).and_then(|p| p.value.value.as_ref().and_then(|v| v.as_i64()))
    })
}

pub async fn extract(page: &Page, cfg: &ExtractConfig) -> Result<(String, RefRegistry), BrowserError> {
    let res = page.execute(GetFullAxTreeParams::builder().build()).await
        .map_err(|e| BrowserError::Cdp(format!("a11y: {e}")))?;
    let nodes = res.result.nodes;
    if nodes.is_empty() { return Err(BrowserError::Cdp("empty a11y tree".into())); }

    let map: HashMap<&str, &AxNode> = nodes.iter().map(|n| (n.node_id.as_ref(), n)).collect();
    let root: &str = nodes[0].node_id.as_ref();

    let mut out = String::with_capacity(cfg.max_chars.min(32_768));
    let mut refs = RefRegistry::default();
    let mut counter = 1u32;
    let mut nth_map: HashMap<(String, String), u32> = HashMap::new();
    let mut emitted_nodes = 0usize;

    emit(root, &map, 0, cfg, &mut out, &mut refs, &mut counter, &mut nth_map, &mut emitted_nodes);

    Ok((out, refs))
}

/// Returns true if this subtree emitted anything (for compact pruning).
fn emit(
    id: &str,
    map: &HashMap<&str, &AxNode>,
    depth: usize,
    cfg: &ExtractConfig,
    out: &mut String,
    refs: &mut RefRegistry,
    counter: &mut u32,
    nth: &mut HashMap<(String, String), u32>,
    emitted: &mut usize,
) -> bool {
    if out.len() >= cfg.max_chars || *emitted >= cfg.max_nodes || depth > cfg.max_depth { return false; }

    let Some(node) = map.get(id) else { return false };

    if node.ignored {
        return emit_children(node, map, depth, cfg, out, refs, counter, nth, emitted);
    }

    let role = ax_str(&node.role);
    let name = ax_str(&node.name);
    let value = ax_str(&node.value);
    let cat = classify(role);

    if cat == Role::Skip {
        return emit_children(node, map, depth, cfg, out, refs, counter, nth, emitted);
    }

    let indent = &"                    "[..((depth.min(10)) * 2)];

    match cat {
        Role::Interactive => {
            let ref_id = *counter;
            *counter += 1;
            *emitted += 1;

            let display = if !name.is_empty() { trunc(name, 200) } else { trunc(value, 100) };
            let nth_key = (role.to_string(), trunc(name, 50).to_string());
            let n = nth.entry(nth_key).or_insert(0);
            let nth_val = *n;
            *n += 1;

            let _ = write!(out, "{indent}[e{ref_id}] {role} \"{display}\"");
            if !value.is_empty() && !name.is_empty() && value != name {
                let _ = write!(out, " value=\"{}\"", trunc(value, 100));
            }
            if nth_val > 0 { let _ = write!(out, " [nth={nth_val}]"); }
            emit_state(out, node);
            out.push('\n');

            refs.insert(ref_id, role.into(), display.into(), None, node.backend_dom_node_id.as_ref().map(|b| *b.inner()));

            emit_children(node, map, depth + 1, cfg, out, refs, counter, nth, emitted);
            true
        }
        Role::Text if !cfg.interactive_only => {
            if !name.is_empty() {
                *emitted += 1;
                let _ = writeln!(out, "{indent}{}", trunc(name, 200));
                true
            } else { false }
        }
        Role::Content if !cfg.interactive_only => {
            let mut did = false;
            if !name.is_empty() {
                *emitted += 1;
                let label = if role == "heading" {
                    prop_i64(node, AxPropertyName::Level).map_or_else(|| role.into(), |l| format!("h{l}"))
                } else { role.into() };
                let _ = writeln!(out, "{indent}-- {label}: \"{}\" --", trunc(name, 120));
                did = true;
            }
            did | emit_children(node, map, depth + 1, cfg, out, refs, counter, nth, emitted)
        }
        Role::Structural => {
            if cfg.compact {
                // Bottom-up: emit children first, only then decide if we show the structural label
                let before = out.len();
                let child_did = emit_children(node, map, depth + 1, cfg, out, refs, counter, nth, emitted);
                if child_did && !name.is_empty() && !cfg.interactive_only {
                    let label = format!("{indent}-- {role}: \"{}\" --\n", trunc(name, 80));
                    out.insert_str(before, &label);
                }
                child_did
            } else {
                let did_self = if !name.is_empty() && !cfg.interactive_only {
                    *emitted += 1;
                    let _ = writeln!(out, "{indent}-- {role}: \"{}\" --", trunc(name, 80));
                    true
                } else { false };
                did_self | emit_children(node, map, depth + 1, cfg, out, refs, counter, nth, emitted)
            }
        }
        _ => emit_children(node, map, depth, cfg, out, refs, counter, nth, emitted),
    }
}

fn emit_children(
    node: &AxNode,
    map: &HashMap<&str, &AxNode>,
    depth: usize,
    cfg: &ExtractConfig,
    out: &mut String,
    refs: &mut RefRegistry,
    counter: &mut u32,
    nth: &mut HashMap<(String, String), u32>,
    emitted: &mut usize,
) -> bool {
    let Some(children) = &node.child_ids else { return false };
    let mut any = false;

    // Collapse long lists: show first 10 + last 2, summarize middle
    if children.len() > 15 {
        let indent = &"                    "[..((depth.min(10)) * 2)];
        for (i, c) in children.iter().enumerate() {
            if out.len() >= cfg.max_chars || *emitted >= cfg.max_nodes { break; }
            if i < 10 || i >= children.len() - 2 {
                any |= emit(c.as_ref(), map, depth, cfg, out, refs, counter, nth, emitted);
            } else if i == 10 {
                let _ = writeln!(out, "{indent}... {} more items ...", children.len() - 12);
            }
        }
    } else {
        for c in children {
            if out.len() >= cfg.max_chars || *emitted >= cfg.max_nodes { break; }
            any |= emit(c.as_ref(), map, depth, cfg, out, refs, counter, nth, emitted);
        }
    }
    any
}

fn emit_state(out: &mut String, node: &AxNode) {
    let mut s = Vec::new();
    if prop_bool(node, AxPropertyName::Disabled) { s.push("disabled"); }
    if prop_bool(node, AxPropertyName::Focused) { s.push("focused"); }
    if let Some(v) = prop_str(node, AxPropertyName::Expanded) {
        s.push(if v == "true" || v == "" { "expanded" } else { "collapsed" });
    }
    // Expanded can also be bool
    if s.last() != Some(&"expanded") && s.last() != Some(&"collapsed") {
        if let Some(ps) = &node.properties {
            if let Some(p) = ps.iter().find(|p| p.name == AxPropertyName::Expanded) {
                if let Some(b) = p.value.value.as_ref().and_then(|v| v.as_bool()) {
                    s.push(if b { "expanded" } else { "collapsed" });
                }
            }
        }
    }
    if let Some(v) = prop_str(node, AxPropertyName::Checked) {
        match v { "true" => s.push("checked"), "mixed" => s.push("mixed"), _ => {} }
    }
    if prop_bool(node, AxPropertyName::Required) { s.push("required"); }
    if !s.is_empty() {
        let _ = write!(out, " [{}]", s.join(", "));
    }
}

fn trunc(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}
