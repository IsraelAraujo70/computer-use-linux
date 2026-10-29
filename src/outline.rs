//! Text views of an accessibility snapshot: a one-line-per-element outline and
//! a query matcher. Both read the same compacted node list that element
//! indices refer to, so `[index]` in a line is what `click` and friends take.

use crate::atspi_tree::{AccessibilityAction, AccessibilityNode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

const MAX_NAME_CHARS: usize = 80;
const MAX_TEXT_CHARS: usize = 120;

/// States worth printing; the rest (`enabled`, `visible`, `showing`, ...) hold
/// for nearly every node and only cost tokens.
const SIGNIFICANT_STATES: [&str; 13] = [
    "focused",
    "checked",
    "pressed",
    "selected",
    "expanded",
    "collapsed",
    "indeterminate",
    "required",
    "invalid-entry",
    "modal",
    "busy",
    "read-only",
    "visited",
];

/// How `get_app_state` returns the accessibility tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TreeFormat {
    /// One indented line per element: `[index] role "name" @x,y wxh states`.
    #[default]
    Outline,
    /// The full node objects, with every AT-SPI field.
    Json,
}

/// Renders nodes as indented lines, one per element, in snapshot order.
///
/// With a `viewport` (the window size), elements whose bounds lie wholly
/// outside `0..width` by `0..height`, such as
/// content scrolled away, are left out and counted in a closing line.
pub fn render_outline(nodes: &[AccessibilityNode], viewport: Option<(u32, u32)>) -> String {
    let mut out = String::new();
    let mut omitted = 0;
    for node in nodes {
        if viewport.is_some_and(|viewport| outside(node, viewport)) {
            omitted += 1;
            continue;
        }
        let indent = node_depth_in(nodes, node);
        for _ in 0..indent {
            out.push_str("  ");
        }
        out.push_str(&render_line(node));
        out.push('\n');
    }
    if omitted > 0 {
        out.push_str(&format!(
            "({omitted} elements outside the visible window omitted; find_elements searches them)\n"
        ));
    }
    out
}

/// One element as `wait_for(changed)` compares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementContent {
    /// Every field except the index, parent index, and object path, which
    /// shift whenever an earlier node appears or disappears. Untruncated, so a
    /// change past the outline's display limits still counts.
    key: String,
    /// The outline line without its `[index]` prefix, as reported in diffs.
    line: String,
}

/// The comparable content of each node, in snapshot order.
pub fn element_contents(nodes: &[AccessibilityNode]) -> Vec<ElementContent> {
    nodes
        .iter()
        .map(|node| {
            let mut comparable = node.clone();
            comparable.index = 0;
            comparable.parent_index = None;
            comparable.object_ref.clear();
            let line = render_line(node);
            ElementContent {
                key: serde_json::to_string(&comparable).unwrap_or_default(),
                line: match line.split_once("] ") {
                    Some((_, rest)) => rest.to_string(),
                    None => line,
                },
            }
        })
        .collect()
}

/// Elements only in `after` (`+ `) then elements only in `before` (`- `),
/// counted as multisets, at most `limit` lines in total.
pub fn content_diff(
    before: &[ElementContent],
    after: &[ElementContent],
    limit: usize,
) -> Vec<String> {
    use std::collections::HashMap;
    let mut counts: HashMap<&str, i32> = HashMap::new();
    for element in before {
        *counts.entry(&element.key).or_default() -= 1;
    }
    for element in after {
        *counts.entry(&element.key).or_default() += 1;
    }
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut take = |elements: &[ElementContent], sign: i32, out: &mut Vec<String>, prefix: &str| {
        for element in elements {
            let count = counts.get_mut(element.key.as_str()).expect("counted above");
            if *count * sign > 0 {
                *count -= sign;
                out.push(format!("{prefix}{}", element.line));
            }
        }
    };
    take(after, 1, &mut added, "+ ");
    take(before, -1, &mut removed, "- ");
    added.into_iter().chain(removed).take(limit).collect()
}

fn outside(node: &AccessibilityNode, (width, height): (u32, u32)) -> bool {
    let Some(bounds) = &node.bounds else {
        return false;
    };
    bounds.x >= width as i32
        || bounds.y >= height as i32
        || bounds.x + bounds.width <= 0
        || bounds.y + bounds.height <= 0
}

/// Renders one element without indentation.
pub fn render_line(node: &AccessibilityNode) -> String {
    let mut line = format!("[{}] {}", node.index, node.role);
    let name = clean(node.name.as_deref());
    if let Some(name) = &name {
        let _ = write!(line, " \"{}\"", truncate(name, MAX_NAME_CHARS));
    }
    if let Some(bounds) = &node.bounds {
        let _ = write!(
            line,
            " @{},{} {}x{}",
            bounds.x, bounds.y, bounds.width, bounds.height
        );
    }
    let mut flags: Vec<&str> = node
        .states
        .iter()
        .map(String::as_str)
        .filter(|state| SIGNIFICANT_STATES.contains(state))
        .collect();
    if node.supports_editable_text {
        flags.push("editable");
    }
    if !node.states.iter().any(|state| state == "enabled") && !node.actions.is_empty() {
        flags.push("disabled");
    }
    if !flags.is_empty() {
        let _ = write!(line, " {{{}}}", flags.join(","));
    }
    let actions: Vec<&str> = node
        .actions
        .iter()
        .filter(|action| is_specific_action(action))
        .map(|action| action.name.as_str())
        .collect();
    if !actions.is_empty() {
        let _ = write!(line, " actions={}", actions.join(","));
    }
    if let Some(value) = &node.value {
        match value.text.as_deref().and_then(|text| clean(Some(text))) {
            Some(text) => {
                let _ = write!(line, " value=\"{}\"", truncate(&text, MAX_NAME_CHARS));
            }
            None => {
                let _ = write!(
                    line,
                    " value={}[{}..{}]",
                    value.current, value.minimum, value.maximum
                );
            }
        }
    }
    let text = clean(node.text.as_ref().and_then(|text| text.content.as_deref()));
    if let Some(text) = text.filter(|text| Some(text) != name.as_ref()) {
        let _ = write!(line, " text=\"{}\"", truncate(&text, MAX_TEXT_CHARS));
    }
    let description = clean(node.description.as_deref());
    if let Some(description) = description.filter(|text| Some(text) != name.as_ref()) {
        let _ = write!(line, " desc=\"{}\"", truncate(&description, MAX_NAME_CHARS));
    }
    line
}

/// Short reference to an element for action results: `[id] role "name"`.
pub fn element_label(node: &AccessibilityNode) -> String {
    match clean(node.name.as_deref()) {
        Some(name) => format!(
            "[{}] {} \"{}\"",
            node.index,
            node.role,
            truncate(&name, MAX_NAME_CHARS)
        ),
        None => format!("[{}] {}", node.index, node.role),
    }
}

/// Whether an action says more than "this is a Chromium container": Chromium
/// gives every node `doDefault` and `showContextMenu`, and every descendant of
/// a clickable element `clickAncestor`.
pub fn is_specific_action(action: &AccessibilityAction) -> bool {
    !matches!(
        action.name.to_ascii_lowercase().as_str(),
        "" | "dodefault" | "showcontextmenu" | "clickancestor"
    )
}

/// Filter for `find_elements` and `wait_for`. Every given field must match;
/// string fields are case-insensitive substrings.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ElementQuery {
    /// Role substring, for example `button`, `entry`, `link`, `check box`.
    #[serde(default)]
    pub role: Option<String>,
    /// Substring of the element's accessible name.
    #[serde(default)]
    pub name: Option<String>,
    /// Substring searched in the name, description, text content, and value text.
    #[serde(default)]
    pub text: Option<String>,
    /// States the element must have, for example `["focused"]` or `["checked"]`.
    #[serde(default)]
    pub states: Option<Vec<String>>,
}

impl ElementQuery {
    pub fn is_empty(&self) -> bool {
        blank(self.role.as_deref())
            && blank(self.name.as_deref())
            && blank(self.text.as_deref())
            && self.states.as_ref().is_none_or(Vec::is_empty)
    }

    pub fn matches(&self, node: &AccessibilityNode) -> bool {
        if let Some(role) = needle(self.role.as_deref()) {
            if !node.role.to_lowercase().contains(&role) {
                return false;
            }
        }
        if let Some(name) = needle(self.name.as_deref()) {
            if !contains(node.name.as_deref(), &name) {
                return false;
            }
        }
        if let Some(text) = needle(self.text.as_deref()) {
            let found = contains(node.name.as_deref(), &text)
                || contains(node.description.as_deref(), &text)
                || contains(node.text.as_ref().and_then(|t| t.content.as_deref()), &text)
                || contains(node.value.as_ref().and_then(|v| v.text.as_deref()), &text);
            if !found {
                return false;
            }
        }
        if let Some(states) = &self.states {
            let all_present = states.iter().all(|wanted| {
                node.states
                    .iter()
                    .any(|state| state.eq_ignore_ascii_case(wanted.trim()))
            });
            if !all_present {
                return false;
            }
        }
        true
    }
}

fn node_depth_in(nodes: &[AccessibilityNode], node: &AccessibilityNode) -> usize {
    let mut depth = 0;
    let mut parent = node.parent_index;
    while let Some(index) = parent {
        depth += 1;
        parent = nodes.get(index as usize).and_then(|p| p.parent_index);
    }
    depth
}

/// Trims, collapses whitespace, and drops U+FFFC (embedded-object marker).
pub fn clean(value: Option<&str>) -> Option<String> {
    let cleaned = value?
        .replace('\u{fffc}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Cuts `value` to `max_chars` with an ellipsis; double quotes become single.
pub fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.replace('"', "'");
    }
    let mut cut: String = value.chars().take(max_chars).collect();
    cut.push('…');
    cut.replace('"', "'")
}

fn blank(value: Option<&str>) -> bool {
    value.is_none_or(|value| value.trim().is_empty())
}

fn needle(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase)
}

fn contains(haystack: Option<&str>, needle: &str) -> bool {
    haystack.is_some_and(|haystack| haystack.to_lowercase().contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atspi_tree::{AccessibilityText, Bounds};

    fn node(index: u32, parent: Option<u32>, role: &str, name: Option<&str>) -> AccessibilityNode {
        AccessibilityNode {
            index,
            parent_index: parent,
            depth: 0,
            object_ref: format!(":1.1/{index}"),
            role: role.to_string(),
            name: name.map(str::to_string),
            description: None,
            child_count: 0,
            bounds: None,
            states: vec!["enabled".into(), "visible".into(), "showing".into()],
            actions: Vec::new(),
            value: None,
            text: None,
            supports_editable_text: false,
        }
    }

    fn action(name: &str) -> AccessibilityAction {
        AccessibilityAction {
            index: 0,
            name: name.into(),
            description: String::new(),
            keybinding: String::new(),
        }
    }

    #[test]
    fn outline_indents_by_kept_parent_and_hides_noise() {
        let mut button = node(1, Some(0), "button", Some("Save"));
        button.bounds = Some(Bounds {
            x: 10,
            y: 20,
            width: 80,
            height: 24,
        });
        button.states.push("focused".into());
        button.actions = vec![action("click"), action("showContextMenu")];
        let mut entry = node(2, Some(0), "entry", None);
        entry.supports_editable_text = true;
        entry.text = Some(AccessibilityText {
            caret_offset: Some(0),
            character_count: 5,
            content: Some("hello\u{fffc}".into()),
            selections: Vec::new(),
            truncated: false,
        });

        let mut scrolled_away = node(3, Some(0), "static", Some("Old message"));
        scrolled_away.bounds = Some(Bounds {
            x: 0,
            y: 5000,
            width: 100,
            height: 20,
        });

        let outline = render_outline(
            &[
                node(0, None, "frame", Some("App")),
                button,
                entry,
                scrolled_away,
            ],
            Some((800, 600)),
        );

        assert_eq!(
            outline,
            "[0] frame \"App\"\n  [1] button \"Save\" @10,20 80x24 {focused} actions=click\n  [2] entry {editable} text=\"hello\"\n(1 elements outside the visible window omitted; find_elements searches them)\n"
        );
    }

    #[test]
    fn disabled_is_shown_only_for_actionable_nodes_missing_enabled() {
        let mut button = node(0, None, "button", Some("Go"));
        button.states.clear();
        button.actions = vec![action("press")];
        assert!(render_line(&button).contains("{disabled}"));

        let mut label = node(1, None, "label", Some("Hi"));
        label.states.clear();
        assert!(!render_line(&label).contains("disabled"));
    }

    #[test]
    fn diff_reports_added_and_removed_elements_as_multisets() {
        let before = element_contents(&[
            node(0, None, "button", Some("Send")),
            node(1, None, "static", Some("hi")),
        ]);
        // Indices shift when a node is inserted; that alone is no change.
        let after = element_contents(&[
            node(0, None, "button", Some("Send")),
            node(1, None, "static", Some("hi")),
            node(2, None, "static", Some("hi")),
            node(3, None, "dialog", Some("Sent")),
        ]);

        assert_eq!(
            content_diff(&before, &after, 10),
            vec!["+ static \"hi\"", "+ dialog \"Sent\""]
        );
        assert_eq!(content_diff(&after, &before, 1), vec!["- static \"hi\""]);
        assert!(content_diff(&before, &before, 10).is_empty());
    }

    #[test]
    fn diff_sees_changes_past_the_display_limit() {
        let long = "x".repeat(MAX_NAME_CHARS + 20);
        let before = element_contents(&[node(0, None, "static", Some(&format!("{long}a")))]);
        let after = element_contents(&[node(0, None, "static", Some(&format!("{long}b")))]);

        assert_eq!(before[0].line, after[0].line);
        assert_eq!(content_diff(&before, &after, 10).len(), 2);
    }

    #[test]
    fn query_requires_every_given_field() {
        let mut checkbox = node(0, None, "check box", Some("Remember me"));
        checkbox.states.push("checked".into());
        let query =
            |role: Option<&str>, text: Option<&str>, states: Option<Vec<&str>>| ElementQuery {
                role: role.map(str::to_string),
                name: None,
                text: text.map(str::to_string),
                states: states.map(|s| s.into_iter().map(str::to_string).collect()),
            };

        assert!(query(Some("CHECK"), Some("remember"), Some(vec!["checked"])).matches(&checkbox));
        assert!(!query(Some("button"), Some("remember"), None).matches(&checkbox));
        assert!(!query(None, None, Some(vec!["focused"])).matches(&checkbox));
        assert!(query(None, None, None).is_empty());
    }
}
