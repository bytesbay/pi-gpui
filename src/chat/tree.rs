//! The session tree from RPC `get_tree`, flattened into lines for the tree
//! view.
//!
//! Only entries a person cares about become lines: user messages, assistant
//! answers with text, compactions and branch summaries. Tool calls, tool
//! results, model changes and other bookkeeping are folded away; their
//! children attach to the nearest visible ancestor.
//!
//! A linear stretch of conversation stays at one indentation level; only a
//! branch point indents, drawn with `├`/`└` connectors. Of the branches at a
//! point, the one holding the current position comes last, so the active
//! conversation reads straight down to the end of the list.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NodeKind {
    User,
    Assistant,
    Compaction,
    BranchSummary,
}

/// One column of the drawing to the left of a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Gutter {
    /// A branch above continues below this line.
    Pipe,
    Blank,
    /// This line starts a branch that has siblings below it.
    Tee,
    /// This line starts the last branch at its point.
    Corner,
}

#[derive(Clone, Debug)]
pub(super) struct TreeLine {
    pub id: String,
    pub kind: NodeKind,
    /// Full text, for the preview and for editing a user message again.
    pub text: String,
    pub gutter: Vec<Gutter>,
    /// On the path from the root to the current position.
    pub active: bool,
    /// The current position (the last visible entry of the active path).
    pub current: bool,
    pub label: Option<String>,
}

struct Visible {
    id: String,
    kind: NodeKind,
    text: String,
    label: Option<String>,
    active: bool,
    children: Vec<Visible>,
}

/// Flattens a `get_tree` response. Returns the lines and the index of the
/// current position.
pub(super) fn flatten(data: &Value) -> (Vec<TreeLine>, Option<usize>) {
    let roots = data["tree"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut parents = HashMap::new();
    for root in roots {
        collect_parents(root, &mut parents);
    }
    let mut active = HashSet::new();
    let mut cursor = data["leafId"].as_str().map(str::to_string);
    while let Some(id) = cursor {
        cursor = parents.get(&id).cloned().flatten();
        active.insert(id);
    }

    let mut visible: Vec<Visible> = roots
        .iter()
        .flat_map(|root| convert(root, &active))
        .collect();
    order(&mut visible);
    let mut lines = Vec::new();
    emit_siblings(&visible, &[], &mut lines);

    let current = lines.iter().rposition(|line| line.active);
    if let Some(index) = current {
        lines[index].current = true;
    }
    (lines, current)
}

fn collect_parents(node: &Value, parents: &mut HashMap<String, Option<String>>) {
    let entry = &node["entry"];
    if let Some(id) = entry["id"].as_str() {
        parents.insert(
            id.to_string(),
            entry["parentId"].as_str().map(str::to_string),
        );
    }
    for child in node["children"].as_array().into_iter().flatten() {
        collect_parents(child, parents);
    }
}

fn convert(node: &Value, active: &HashSet<String>) -> Vec<Visible> {
    let children: Vec<Visible> = node["children"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|child| convert(child, active))
        .collect();
    let entry = &node["entry"];
    let Some((kind, text)) = describe(entry) else {
        return children;
    };
    let id = entry["id"].as_str().unwrap_or_default().to_string();
    vec![Visible {
        active: active.contains(&id),
        id,
        kind,
        text,
        label: node["label"].as_str().map(str::to_string),
        children,
    }]
}

fn describe(entry: &Value) -> Option<(NodeKind, String)> {
    match entry["type"].as_str()? {
        "message" => {
            let message = &entry["message"];
            match message["role"].as_str()? {
                "user" => Some((NodeKind::User, crate::sessions::message_text(message))),
                "assistant" => {
                    let text: Vec<&str> = message["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|block| block["type"] == "text")
                        .filter_map(|block| block["text"].as_str())
                        .filter(|text| !text.trim().is_empty())
                        .collect();
                    (!text.is_empty()).then(|| (NodeKind::Assistant, text.join("\n\n")))
                }
                _ => None,
            }
        }
        "compaction" => Some((
            NodeKind::Compaction,
            entry["summary"].as_str().unwrap_or_default().to_string(),
        )),
        "branch_summary" => Some((
            NodeKind::BranchSummary,
            entry["summary"].as_str().unwrap_or_default().to_string(),
        )),
        _ => None,
    }
}

/// Inactive branches first, the active one last, recursively.
fn order(nodes: &mut [Visible]) {
    nodes.sort_by_key(|node| node.active);
    for node in nodes {
        order(&mut node.children);
    }
}

fn emit_siblings(nodes: &[Visible], gutter: &[Gutter], lines: &mut Vec<TreeLine>) {
    if let [only] = nodes {
        emit_chain(only, gutter.to_vec(), gutter, lines);
        return;
    }
    for (index, node) in nodes.iter().enumerate() {
        let last = index + 1 == nodes.len();
        let mut own = gutter.to_vec();
        own.push(if last { Gutter::Corner } else { Gutter::Tee });
        let mut below = gutter.to_vec();
        below.push(if last { Gutter::Blank } else { Gutter::Pipe });
        emit_chain(node, own, &below, lines);
    }
}

/// `node` and its single-child descendants share one level; `own` is the
/// gutter of `node` itself, `below` the gutter of everything after it.
fn emit_chain(node: &Visible, own: Vec<Gutter>, below: &[Gutter], lines: &mut Vec<TreeLine>) {
    lines.push(line(node, own));
    let mut current = node;
    while let [only] = current.children.as_slice() {
        lines.push(line(only, below.to_vec()));
        current = only;
    }
    if !current.children.is_empty() {
        emit_siblings(&current.children, below, lines);
    }
}

fn line(node: &Visible, gutter: Vec<Gutter>) -> TreeLine {
    TreeLine {
        id: node.id.clone(),
        kind: node.kind,
        text: node.text.clone(),
        gutter,
        active: node.active,
        current: false,
        label: node.label.clone(),
    }
}

/// The first line of the branch that contains `index` (where its connector
/// is), or the closest branch point above.
pub(super) fn branch_start(lines: &[TreeLine], index: usize) -> usize {
    let depth = lines.get(index).map_or(0, |line| line.gutter.len());
    (0..index)
        .rev()
        .find(|&candidate| {
            let line = &lines[candidate];
            line.gutter.len() < depth
                || (line.gutter.len() == depth
                    && matches!(line.gutter.last(), Some(Gutter::Tee | Gutter::Corner)))
        })
        .unwrap_or(0)
}

/// The next line that starts a branch (has a connector), after `index`.
pub(super) fn next_branch(lines: &[TreeLine], index: usize) -> Option<usize> {
    (index + 1..lines.len()).find(|&candidate| {
        matches!(
            lines[candidate].gutter.last(),
            Some(Gutter::Tee | Gutter::Corner)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(id: &str, parent: Option<&str>, role: &str, text: &str) -> Value {
        let content = if role == "assistant" {
            json!([{ "type": "text", "text": text }])
        } else {
            json!(text)
        };
        json!({ "type": "message", "id": id, "parentId": parent, "message": { "role": role, "content": content } })
    }

    fn node(entry: Value, children: Vec<Value>) -> Value {
        json!({ "entry": entry, "children": children })
    }

    /// u1 → a1 → (u2 → a2) | (u3 → tool result → a3), leaf at a3.
    fn sample() -> Value {
        let tool_result = json!({ "type": "message", "id": "t3", "parentId": "u3", "message": { "role": "toolResult", "content": [] } });
        json!({
            "leafId": "a3",
            "tree": [node(message("u1", None, "user", "first"), vec![
                node(message("a1", Some("u1"), "assistant", "answer"), vec![
                    node(message("u3", Some("a1"), "user", "branch B"), vec![
                        node(tool_result, vec![
                            node(message("a3", Some("t3"), "assistant", "answer B"), vec![]),
                        ]),
                    ]),
                    node(message("u2", Some("a1"), "user", "branch A"), vec![
                        node(message("a2", Some("u2"), "assistant", "answer A"), vec![]),
                    ]),
                ]),
            ])],
        })
    }

    #[test]
    fn branches_indent_and_the_active_one_comes_last() {
        let (lines, current) = flatten(&sample());
        let ids: Vec<&str> = lines.iter().map(|line| line.id.as_str()).collect();
        // The tool result is folded away; branch A (inactive) comes first.
        assert_eq!(ids, ["u1", "a1", "u2", "a2", "u3", "a3"]);
        assert_eq!(lines[0].gutter, []);
        assert_eq!(lines[2].gutter, [Gutter::Tee]);
        assert_eq!(lines[3].gutter, [Gutter::Pipe]);
        assert_eq!(lines[4].gutter, [Gutter::Corner]);
        assert_eq!(lines[5].gutter, [Gutter::Blank]);
        assert_eq!(current, Some(5));
        assert!(lines[5].current && lines[4].active && !lines[2].active);
    }

    #[test]
    fn branch_keys_jump_between_branch_starts() {
        let (lines, _) = flatten(&sample());
        assert_eq!(next_branch(&lines, 0), Some(2));
        assert_eq!(next_branch(&lines, 2), Some(4));
        assert_eq!(branch_start(&lines, 5), 4);
        assert_eq!(branch_start(&lines, 3), 2);
    }
}
