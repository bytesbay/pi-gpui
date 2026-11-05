//! The session tree from RPC `get_entries`, flattened into lines for the tree
//! view. `get_tree` is not used: it nests one level per entry, which a long
//! session pushes past serde_json's recursion limit.
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
    /// Indices into the node list, in append order.
    children: Vec<usize>,
}

/// Flattens a `get_entries` response. Returns the lines and the index of the
/// current position.
pub(super) fn flatten(data: &Value) -> (Vec<TreeLine>, Option<usize>) {
    let entries = data["entries"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();

    let mut parents: HashMap<&str, Option<&str>> = HashMap::new();
    let mut labels: HashMap<&str, &str> = HashMap::new();
    for entry in entries {
        let Some(id) = entry["id"].as_str() else {
            continue;
        };
        let parent = entry["parentId"].as_str().filter(|parent| *parent != id);
        parents.insert(id, parent);
        if entry["type"] == "label"
            && let Some(target) = entry["targetId"].as_str()
        {
            match entry["label"].as_str().filter(|label| !label.is_empty()) {
                Some(label) => labels.insert(target, label),
                None => labels.remove(target),
            };
        }
    }

    // The path from the leaf to the root. Bounded in case of a parent cycle.
    let mut active = HashSet::new();
    let mut cursor = data["leafId"].as_str();
    while let Some(id) = cursor {
        if !active.insert(id) {
            break;
        }
        cursor = parents.get(id).copied().flatten();
    }

    let mut nodes = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut entry_parents = Vec::new();
    for entry in entries {
        let Some(id) = entry["id"].as_str() else {
            continue;
        };
        let Some((kind, text)) = describe(entry) else {
            continue;
        };
        index.insert(id, nodes.len());
        entry_parents.push(parents.get(id).copied().flatten());
        nodes.push(Visible {
            id: id.to_string(),
            kind,
            text,
            label: labels.get(id).map(|label| label.to_string()),
            active: active.contains(id),
            children: Vec::new(),
        });
    }

    // Attach each visible node to its nearest visible ancestor.
    let mut nearest: HashMap<&str, Option<usize>> = HashMap::new();
    let mut roots = Vec::new();
    for (child, parent) in entry_parents.into_iter().enumerate() {
        match nearest_visible(parent, &parents, &index, &mut nearest) {
            Some(parent) if parent != child => nodes[parent].children.push(child),
            _ => roots.push(child),
        }
    }

    // Inactive branches first, the active one last.
    let order = |list: &mut Vec<usize>, nodes: &[Visible]| {
        list.sort_by_key(|&node| nodes[node].active);
    };
    order(&mut roots, &nodes);
    for node in 0..nodes.len() {
        let mut children = std::mem::take(&mut nodes[node].children);
        order(&mut children, &nodes);
        nodes[node].children = children;
    }

    let mut lines = Vec::new();
    emit_siblings(&nodes, &roots, &[], &mut lines);

    let current = lines.iter().rposition(|line| line.active);
    if let Some(index) = current {
        lines[index].current = true;
    }
    (lines, current)
}

/// The visible node at or above `start`, walking up through hidden entries.
fn nearest_visible<'a>(
    start: Option<&'a str>,
    parents: &HashMap<&'a str, Option<&'a str>>,
    index: &HashMap<&'a str, usize>,
    memo: &mut HashMap<&'a str, Option<usize>>,
) -> Option<usize> {
    let mut walked = Vec::new();
    let mut cursor = start;
    let found = loop {
        let Some(id) = cursor else { break None };
        if let Some(&found) = memo.get(id) {
            break found;
        }
        if let Some(&found) = index.get(id) {
            break Some(found);
        }
        if walked.len() > parents.len() {
            break None;
        }
        walked.push(id);
        cursor = parents.get(id).copied().flatten();
    };
    for id in walked {
        memo.insert(id, found);
    }
    found
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

fn emit_siblings(nodes: &[Visible], list: &[usize], gutter: &[Gutter], lines: &mut Vec<TreeLine>) {
    if let [only] = list {
        emit_chain(nodes, *only, gutter.to_vec(), gutter, lines);
        return;
    }
    for (index, &node) in list.iter().enumerate() {
        let last = index + 1 == list.len();
        let mut own = gutter.to_vec();
        own.push(if last { Gutter::Corner } else { Gutter::Tee });
        let mut below = gutter.to_vec();
        below.push(if last { Gutter::Blank } else { Gutter::Pipe });
        emit_chain(nodes, node, own, &below, lines);
    }
}

/// `node` and its single-child descendants share one level; `own` is the
/// gutter of `node` itself, `below` the gutter of everything after it.
fn emit_chain(
    nodes: &[Visible],
    node: usize,
    own: Vec<Gutter>,
    below: &[Gutter],
    lines: &mut Vec<TreeLine>,
) {
    lines.push(line(&nodes[node], own));
    let mut current = node;
    while let [only] = nodes[current].children.as_slice() {
        lines.push(line(&nodes[*only], below.to_vec()));
        current = *only;
    }
    if !nodes[current].children.is_empty() {
        emit_siblings(nodes, &nodes[current].children, below, lines);
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

    /// u1 → a1 → (u2 → a2) | (u3 → tool result → a3), leaf at a3.
    fn sample() -> Value {
        let tool_result = json!({ "type": "message", "id": "t3", "parentId": "u3", "message": { "role": "toolResult", "content": [] } });
        json!({
            "leafId": "a3",
            "entries": [
                message("u1", None, "user", "first"),
                message("a1", Some("u1"), "assistant", "answer"),
                message("u2", Some("a1"), "user", "branch A"),
                message("a2", Some("u2"), "assistant", "answer A"),
                message("u3", Some("a1"), "user", "branch B"),
                tool_result,
                message("a3", Some("t3"), "assistant", "answer B"),
                { "type": "label", "id": "l1", "parentId": "a3", "targetId": "u2", "label": "idea" },
            ],
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
        assert_eq!(lines[2].label.as_deref(), Some("idea"));
    }

    #[test]
    fn a_long_linear_session_flattens() {
        let mut entries = Vec::new();
        let mut parent: Option<String> = None;
        for index in 0..5000 {
            let id = format!("m{index}");
            let role = if index % 2 == 0 { "user" } else { "assistant" };
            entries.push(message(&id, parent.as_deref(), role, "text"));
            parent = Some(id);
        }
        let data = json!({ "leafId": parent, "entries": entries });
        let (lines, current) = flatten(&data);
        assert_eq!(lines.len(), 5000);
        assert_eq!(current, Some(4999));
        assert!(lines.iter().all(|line| line.gutter.is_empty()));
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
