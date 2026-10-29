//! What the agent sees of a snapshot, and what changed between two of them.
//!
//! A browser window shows its title, URL and tabs, then only the visible web
//! document; other browser UI stays out of outlines and change reports but
//! remains in the snapshot, so `find_elements` and element ids still reach it.
//! Every view is drawn depth-first, children under their parent.

use crate::atspi_tree::{AccessibilityNode, Bounds};
use crate::outline::{clean, element_label, render_line, truncate};
use std::collections::HashMap;

/// Change lines above this many give way to the whole current outline.
pub const MAX_CHANGE_LINES: usize = 40;

/// How a snapshot is turned into a view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ViewOptions {
    /// Show the browser UI (toolbars, bubbles) around a web document too.
    pub include_browser_ui: bool,
}

/// One element line of a view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewEntry {
    pub id: u32,
    /// Id of the nearest ancestor shown in the same view.
    pub parent: Option<u32>,
    pub depth: usize,
    /// `[id] role "name" @x,y wxh {states} ...`, unindented.
    pub line: String,
    /// `[id] role "name"`, for elements reported gone.
    pub label: String,
    /// Everything but the position, untruncated: a change here is a `~`.
    key: String,
}

/// The visible part of a snapshot, as outlines and change reports print it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageView {
    /// Window title (the frame's name).
    pub title: Option<String>,
    /// Address bar text, when a browser document scoped the view.
    pub url: Option<String>,
    /// Lines of the browser's page tabs.
    pub tabs: Vec<String>,
    /// True when a web document scoped the view.
    pub browser: bool,
    /// Visible elements, depth-first.
    pub entries: Vec<ViewEntry>,
    /// Elements left out because they lie outside the visible area.
    pub omitted: usize,
    /// The element holding keyboard focus, with its text when editable.
    pub focus: Option<String>,
}

impl PageView {
    /// Elements with names, text or actions: how much of the app the tree
    /// shows, which decides whether a screenshot adds anything.
    pub fn content_elements(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.line.contains('"') || entry.line.contains("actions="))
            .count()
    }

    /// The outline: tabs and a `--- page ---` rule for browsers, then one
    /// indented line per visible element, then the omitted count.
    pub fn outline(&self) -> String {
        let mut out = String::new();
        for tab in &self.tabs {
            out.push_str("tab ");
            out.push_str(tab);
            out.push('\n');
        }
        if self.browser {
            out.push_str("--- page ---\n");
        }
        for entry in &self.entries {
            for _ in 0..entry.depth {
                out.push_str("  ");
            }
            out.push_str(&entry.line);
            out.push('\n');
        }
        if self.omitted > 0 {
            out.push_str(&format!(
                "({} elements outside the visible window omitted; find_elements searches them)\n",
                self.omitted
            ));
        }
        out
    }
}

/// Builds the view of a snapshot whose `index`/`parent_index` are element
/// ids. With a web document present (and browser UI not requested), the view
/// is that document's subtree; otherwise the whole tree. Elements whose
/// bounds lie wholly outside the visible area (the document viewport, else
/// the window frame) are omitted and counted.
pub fn page_view(nodes: &[AccessibilityNode], options: ViewOptions) -> PageView {
    let tree = Tree::new(nodes);
    let document = (!options.include_browser_ui)
        .then(|| tree.visible_document())
        .flatten();
    let frame = document
        .and_then(|doc| tree.ancestor_with_role(doc, &["frame", "window", "dialog"]))
        .or_else(|| tree.first_with_role(&["frame", "window"]));
    let visible = document
        .and_then(|doc| nodes[doc].bounds.clone())
        .or_else(|| frame.and_then(|frame| nodes[frame].bounds.clone()));

    let roots = match document {
        Some(doc) => vec![doc],
        None => tree.roots.clone(),
    };
    let mut entries = Vec::new();
    let mut omitted = 0;
    // (position, depth, nearest shown ancestor that carries text, nearest
    // shown ancestor)
    let mut stack: Vec<(usize, usize, Option<usize>, Option<u32>)> = roots
        .into_iter()
        .rev()
        .map(|at| (at, 0, None, None))
        .collect();
    let mut visited = vec![false; nodes.len()];
    while let Some((at, depth, context, parent)) = stack.pop() {
        if std::mem::replace(&mut visited[at], true) {
            continue;
        }
        let node = &nodes[at];
        if tree.text_only(at)
            && context.is_some_and(|context| repeats_text_of(node, &nodes[context]))
        {
            // A text leaf whose words its ancestor's line already shows.
            continue;
        }
        let shown = visible.as_ref().is_none_or(|area| !outside(node, area));
        if shown {
            entries.push(ViewEntry {
                id: node.index,
                parent,
                depth,
                line: render_line(node),
                label: element_label(node),
                key: content_key(node),
            });
        } else {
            omitted += 1;
        }
        let child_depth = if shown { depth + 1 } else { depth };
        let child_context =
            if shown && (node_text(node).is_some() || clean(node.name.as_deref()).is_some()) {
                Some(at)
            } else {
                context
            };
        let child_parent = if shown { Some(node.index) } else { parent };
        for &child in tree.children(at).iter().rev() {
            stack.push((child, child_depth, child_context, child_parent));
        }
    }

    PageView {
        title: frame.and_then(|frame| clean(nodes[frame].name.as_deref())),
        url: document.and_then(|doc| tree.address_bar(doc)),
        tabs: document.map(|doc| tree.page_tabs(doc)).unwrap_or_default(),
        browser: document.is_some(),
        entries,
        omitted,
        focus: tree.focused().map(|at| describe_focus(&nodes[at])),
    }
}

/// The screen area where element `id` can be seen: its web document's
/// viewport when it is inside the visible document, else its window frame.
/// `None` when the snapshot shows neither.
pub fn visible_area(nodes: &[AccessibilityNode], id: u32) -> Option<Bounds> {
    let tree = Tree::new(nodes);
    let target = nodes.iter().position(|node| node.index == id);
    if let (Some(document), Some(target)) = (tree.visible_document(), target) {
        if tree.inside(target, document) {
            return nodes[document].bounds.clone();
        }
    }
    target
        .and_then(|target| tree.ancestor_with_role(target, &["frame", "window", "dialog"]))
        .or_else(|| tree.first_with_role(&["frame", "window"]))
        .and_then(|frame| nodes[frame].bounds.clone())
}

/// The part of a scrollable element on screen: its bounds cut to where it
/// can be seen (`visible_area`). Without `id`, the visible web document,
/// else the first window frame.
pub fn shown_part(nodes: &[AccessibilityNode], id: Option<u32>) -> Option<Bounds> {
    match id {
        Some(id) => {
            let node = nodes.iter().find(|node| node.index == id)?;
            let bounds = node.bounds.as_ref()?;
            match visible_area(nodes, id) {
                Some(area) => intersect(bounds, &area),
                None => Some(bounds.clone()),
            }
        }
        None => {
            let tree = Tree::new(nodes);
            tree.visible_document()
                .and_then(|document| nodes[document].bounds.clone())
                .or_else(|| {
                    tree.first_with_role(&["frame", "window"])
                        .and_then(|frame| nodes[frame].bounds.clone())
                })
        }
    }
}

/// The overlap of two rectangles, `None` when they do not overlap.
pub fn intersect(a: &Bounds, b: &Bounds) -> Option<Bounds> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    (right > x && bottom > y).then(|| Bounds {
        x,
        y,
        width: right - x,
        height: bottom - y,
    })
}

/// What changed from the view the agent last saw to the current one: `+`
/// new elements, `-` gone ones, `~` same id with different content (moves
/// alone do not count), then the focus, and the title and URL when they
/// changed. Over `MAX_CHANGE_LINES` lines the whole current outline is
/// printed instead.
pub fn change_report(before: &PageView, after: &PageView) -> String {
    let before_keys: HashMap<u32, &ViewEntry> = before
        .entries
        .iter()
        .map(|entry| (entry.id, entry))
        .collect();
    let after_ids: std::collections::HashSet<u32> =
        after.entries.iter().map(|entry| entry.id).collect();
    let mut added = Vec::new();
    let mut changed = Vec::new();
    for entry in &after.entries {
        match before_keys.get(&entry.id) {
            None => added.push(format!("+ {}", entry.line)),
            Some(old) if old.key != entry.key => changed.push(format!("~ {}", entry.line)),
            Some(_) => {}
        }
    }
    // A removed container stands for everything inside it.
    let gone: std::collections::HashSet<u32> = before
        .entries
        .iter()
        .filter(|entry| !after_ids.contains(&entry.id))
        .map(|entry| entry.id)
        .collect();
    let mut inside_gone: HashMap<u32, usize> = HashMap::new();
    let mut removed = Vec::new();
    for entry in before
        .entries
        .iter()
        .filter(|entry| gone.contains(&entry.id))
    {
        let mut top = entry.id;
        let mut parent = entry.parent;
        while let Some(id) = parent.filter(|id| gone.contains(id)) {
            top = id;
            parent = before_keys.get(&id).and_then(|entry| entry.parent);
        }
        if top == entry.id {
            removed.push(entry);
        } else {
            *inside_gone.entry(top).or_default() += 1;
        }
    }
    let removed: Vec<String> = removed
        .into_iter()
        .map(|entry| match inside_gone.get(&entry.id) {
            Some(count) => format!("- {} (and {count} inside)", entry.label),
            None => format!("- {}", entry.label),
        })
        .collect();

    let mut out = String::new();
    let total = added.len() + changed.len() + removed.len();
    if total > MAX_CHANGE_LINES {
        out.push_str(&format!(
            "page changed ({} added, {} removed); current outline:\n",
            added.len(),
            gone.len()
        ));
        out.push_str(&after.outline());
    } else if total == 0 {
        out.push_str("changes: none\n");
    } else {
        out.push_str(&format!("changes ({total}):\n"));
        for line in added.iter().chain(&changed).chain(&removed) {
            out.push_str(line);
            out.push('\n');
        }
    }
    if let Some(focus) = &after.focus {
        out.push_str(&format!("focus: {focus}\n"));
    }
    if after.title != before.title {
        if let Some(title) = &after.title {
            out.push_str(&format!("title: \"{title}\"\n"));
        }
    }
    if after.url != before.url {
        if let Some(url) = &after.url {
            out.push_str(&format!("url: {url}\n"));
        }
    }
    out
}

/// Lower-case words a control or status shows while the app is still
/// working on a request (Portuguese and English, the languages of the apps
/// agents drive here). A changed label ending in "…" or "..." counts too.
const BUSY_WORDS: [&str; 9] = [
    "carregando",
    "salvando",
    "enviando",
    "processando",
    "aguarde",
    "loading",
    "saving",
    "sending",
    "please wait",
];

/// Why the page still looks busy with an action: the settle loop keeps
/// reading until this is gone, so the report includes the action's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    /// What looks busy, e.g. `[57] button "Salvando…" shows a busy label`.
    pub reason: String,
    /// False when the only sign is a button that became disabled: some stay
    /// disabled for good, so the caller may stop once the page goes quiet.
    pub strong: bool,
}

/// What in `after` says the app is still working on the action that
/// followed `before` (elements matched by id): a label that changed to a
/// busy one (`BUSY_WORDS`, or ending in an ellipsis), a new element with a
/// busy word, a newly `busy` element, a new progress bar, or a button that
/// was enabled and is now disabled (weak). `None` when nothing does.
pub fn busy_reason(before: &[AccessibilityNode], after: &[AccessibilityNode]) -> Option<Busy> {
    let earlier: HashMap<u32, &AccessibilityNode> =
        before.iter().map(|node| (node.index, node)).collect();
    let mut weak = None;
    for node in after {
        let old = earlier.get(&node.index).copied();
        let label = busy_label(node);
        let strong = |what: &str| {
            Some(Busy {
                reason: format!("{} {what}", element_label(node)),
                strong: true,
            })
        };
        if has_state(node, "busy") && !old.is_some_and(|old| has_state(old, "busy")) {
            return strong("is busy");
        }
        if node.role.eq_ignore_ascii_case("progress bar") && old.is_none() {
            return strong("appeared");
        }
        let newly_busy_label = match (old, label.as_deref()) {
            (_, None) => false,
            (None, Some(label)) => has_busy_word(label),
            (Some(old), Some(label)) => {
                busy_label(old).as_deref() != Some(label)
                    && (has_busy_word(label) || label.ends_with('…') || label.ends_with("..."))
            }
        };
        if newly_busy_label {
            return strong("shows a busy label");
        }
        let became_disabled = node.role.to_ascii_lowercase().ends_with("button")
            && !has_state(node, "enabled")
            && old.is_some_and(|old| has_state(old, "enabled"));
        if became_disabled && weak.is_none() {
            weak = Some(Busy {
                reason: format!("{} became disabled", element_label(node)),
                strong: false,
            });
        }
    }
    weak
}

/// The name, else the text, a busy check reads.
fn busy_label(node: &AccessibilityNode) -> Option<String> {
    clean(node.name.as_deref()).or_else(|| node_text(node))
}

fn has_busy_word(label: &str) -> bool {
    let lower = label.to_lowercase();
    BUSY_WORDS.iter().any(|word| lower.contains(word))
}

/// Parent/child links of a snapshot, by position.
struct Tree<'a> {
    nodes: &'a [AccessibilityNode],
    parent: Vec<Option<usize>>,
    children: Vec<Vec<usize>>,
    roots: Vec<usize>,
}

impl<'a> Tree<'a> {
    fn new(nodes: &'a [AccessibilityNode]) -> Self {
        let position: HashMap<u32, usize> = nodes
            .iter()
            .enumerate()
            .map(|(at, node)| (node.index, at))
            .rev()
            .collect();
        let parent: Vec<Option<usize>> = nodes
            .iter()
            .enumerate()
            .map(|(at, node)| {
                node.parent_index
                    .and_then(|id| position.get(&id).copied())
                    .filter(|&parent| parent != at)
            })
            .collect();
        let mut children = vec![Vec::new(); nodes.len()];
        let mut roots = Vec::new();
        for (at, parent) in parent.iter().enumerate() {
            match parent {
                Some(parent) => children[*parent].push(at),
                None => roots.push(at),
            }
        }
        Self {
            nodes,
            parent,
            children,
            roots,
        }
    }

    fn children(&self, at: usize) -> &[usize] {
        &self.children[at]
    }

    /// Whether the subtree at `at` is plain text: text roles, no actions.
    fn text_only(&self, at: usize) -> bool {
        let mut pending = vec![at];
        let mut seen = 0;
        while let Some(at) = pending.pop() {
            seen += 1;
            let node = &self.nodes[at];
            if seen > self.nodes.len()
                || !TEXT_LEAF_ROLES.contains(&node.role.as_str())
                || node.actions.iter().any(crate::outline::is_specific_action)
            {
                return false;
            }
            pending.extend(self.children(at));
        }
        true
    }

    fn ancestors(&self, at: usize) -> impl Iterator<Item = usize> + '_ {
        let mut current = self.parent[at];
        let mut steps = 0;
        std::iter::from_fn(move || {
            let at = current?;
            steps += 1;
            if steps > self.nodes.len() {
                return None;
            }
            current = self.parent[at];
            Some(at)
        })
    }

    fn role(&self, at: usize) -> String {
        self.nodes[at].role.to_ascii_lowercase()
    }

    fn first_with_role(&self, roles: &[&str]) -> Option<usize> {
        (0..self.nodes.len()).find(|&at| roles.contains(&self.role(at).as_str()))
    }

    fn ancestor_with_role(&self, at: usize, roles: &[&str]) -> Option<usize> {
        self.ancestors(at)
            .find(|&parent| roles.contains(&self.role(parent).as_str()))
    }

    fn inside(&self, at: usize, root: usize) -> bool {
        at == root || self.ancestors(at).any(|parent| parent == root)
    }

    /// The web document the browser shows: a top-level `document web`
    /// (not inside another one, so not an iframe), preferring one that is
    /// showing and has bounds.
    fn visible_document(&self) -> Option<usize> {
        let documents: Vec<usize> = (0..self.nodes.len())
            .filter(|&at| self.role(at) == "document web")
            .filter(|&at| self.ancestor_with_role(at, &["document web"]).is_none())
            .collect();
        documents
            .iter()
            .copied()
            .find(|&at| has_state(&self.nodes[at], "showing") && self.nodes[at].bounds.is_some())
            .or_else(|| documents.first().copied())
    }

    /// The address bar text: an entry outside the document named like an
    /// address bar, else one whose text looks like a URL.
    fn address_bar(&self, document: usize) -> Option<String> {
        let entries: Vec<usize> = (0..self.nodes.len())
            .filter(|&at| self.role(at) == "entry" && !self.inside(at, document))
            .collect();
        let text = |at: usize| {
            clean(
                self.nodes[at]
                    .text
                    .as_ref()
                    .and_then(|text| text.content.as_deref()),
            )
        };
        entries
            .iter()
            .copied()
            .find(|&at| {
                self.nodes[at]
                    .name
                    .as_deref()
                    .is_some_and(|name| name.to_lowercase().contains("address"))
            })
            .and_then(text)
            .or_else(|| {
                entries
                    .iter()
                    .filter_map(|&at| text(at))
                    .find(|text| !text.contains(' ') && text.contains(['.', '/', ':']))
            })
    }

    /// Browser tabs: page tabs outside the document that do not belong to a
    /// bubble or dialog (Chromium's translate bubble has page tabs too).
    fn page_tabs(&self, document: usize) -> Vec<String> {
        (0..self.nodes.len())
            .filter(|&at| self.role(at) == "page tab" && !self.inside(at, document))
            .filter(|&at| {
                !self.ancestors(at).any(|parent| {
                    matches!(
                        self.role(parent).as_str(),
                        "alert" | "dialog" | "notification"
                    ) || (self.role(parent) == "page tab list"
                        && clean(self.nodes[parent].name.as_deref()).is_some())
                })
            })
            .map(|at| render_line(&self.nodes[at]))
            .collect()
    }

    /// The deepest element reporting focus (a document reports it too while
    /// one of its fields holds it in some toolkits).
    fn focused(&self) -> Option<usize> {
        (0..self.nodes.len())
            .filter(|&at| has_state(&self.nodes[at], "focused"))
            .max_by_key(|&at| self.ancestors(at).count())
    }
}

fn has_state(node: &AccessibilityNode, state: &str) -> bool {
    node.states
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(state))
}

/// `[id] role "name"`, plus `value="…"` for an editable element.
fn describe_focus(node: &AccessibilityNode) -> String {
    let mut line = element_label(node);
    if node.supports_editable_text || has_state(node, "editable") {
        let text = clean(node.text.as_ref().and_then(|text| text.content.as_deref()))
            .or_else(|| clean(node.value.as_ref().and_then(|value| value.text.as_deref())))
            .unwrap_or_default();
        line.push_str(&format!(" value=\"{}\"", truncate(&text, 200)));
    }
    line
}

/// Roles that only carry text.
const TEXT_LEAF_ROLES: [&str; 3] = ["static", "text", "label"];

/// The text content an outline line prints for a node, cleaned.
fn node_text(node: &AccessibilityNode) -> Option<String> {
    clean(node.text.as_ref().and_then(|text| text.content.as_deref()))
}

/// Whether a plain-text element's words are already part of the ancestor's
/// printed text, as with Chromium's `static` children of a list item whose
/// text concatenates them. Its id stays reachable through `find_elements`.
fn repeats_text_of(node: &AccessibilityNode, ancestor: &AccessibilityNode) -> bool {
    if has_state(node, "focused") || has_state(node, "editable") {
        return false;
    }
    let Some(own) = clean(node.name.as_deref()).or_else(|| node_text(node)) else {
        return false;
    };
    let shown = format!(
        "{} {}",
        clean(ancestor.name.as_deref()).unwrap_or_default(),
        node_text(ancestor).unwrap_or_default()
    );
    // Lines cut long text, so only text the ancestor's line actually prints
    // counts as repeated.
    let printed: String = shown.chars().take(120).collect();
    printed.contains(&own)
}

/// Whether bounds lie wholly outside the visible area. Elements without
/// bounds are kept: their position is unknown, not off-screen.
fn outside(node: &AccessibilityNode, area: &Bounds) -> bool {
    let Some(bounds) = &node.bounds else {
        return false;
    };
    bounds.x >= area.x + area.width
        || bounds.y >= area.y + area.height
        || bounds.x + bounds.width <= area.x
        || bounds.y + bounds.height <= area.y
}

/// The content of an element apart from its position: a difference here is
/// a `~` change, a move alone is not.
fn content_key(node: &AccessibilityNode) -> String {
    let mut stripped = node.clone();
    stripped.bounds = None;
    let mut key = render_line(&stripped);
    if let Some(text) = node.text.as_ref().and_then(|text| text.content.as_deref()) {
        key.push('\u{1}');
        key.push_str(text);
    }
    if let Some(name) = node.name.as_deref() {
        key.push('\u{1}');
        key.push_str(name);
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atspi_tree::AccessibilityText;

    fn node(id: u32, parent: Option<u32>, role: &str, name: &str) -> AccessibilityNode {
        AccessibilityNode {
            index: id,
            parent_index: parent,
            depth: 0,
            object_ref: format!(":1.1/{id}"),
            role: role.to_string(),
            name: (!name.is_empty()).then(|| name.to_string()),
            description: None,
            child_count: 0,
            bounds: None,
            states: vec!["enabled".into(), "showing".into(), "visible".into()],
            actions: Vec::new(),
            value: None,
            text: None,
            supports_editable_text: false,
        }
    }

    fn at(
        mut node: AccessibilityNode,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> AccessibilityNode {
        node.bounds = Some(Bounds {
            x,
            y,
            width,
            height,
        });
        node
    }

    fn with_text(mut node: AccessibilityNode, text: &str) -> AccessibilityNode {
        node.text = Some(AccessibilityText {
            character_count: text.chars().count() as i32,
            caret_offset: Some(0),
            content: Some(text.to_string()),
            truncated: false,
            selections: Vec::new(),
        });
        node
    }

    /// Chromium's shape: app > frame > (toolbar entries, tabs, document),
    /// plus a translate bubble with page tabs of its own.
    fn browser(page: Vec<AccessibilityNode>) -> Vec<AccessibilityNode> {
        let mut tab = node(5, Some(2), "page tab", "Correio");
        tab.states.push("selected".into());
        let mut nodes = vec![
            node(1, None, "application", "Chromium"),
            at(
                node(2, Some(1), "frame", "Correio - Chromium"),
                0,
                0,
                1000,
                800,
            ),
            with_text(
                node(3, Some(2), "entry", "Address and search bar"),
                "example.com/mail",
            ),
            node(4, Some(2), "button", "Reload"),
            tab,
            node(6, Some(1), "alert", "Translate this page?"),
            node(7, Some(6), "page tab list", "Portuguese"),
            node(8, Some(7), "page tab", "English"),
            at(
                node(10, Some(2), "document web", "Correio"),
                0,
                100,
                1000,
                700,
            ),
        ];
        nodes.extend(page);
        nodes
    }

    #[test]
    fn browser_view_keeps_title_url_tabs_and_only_the_page() {
        let view = page_view(
            &browser(vec![
                at(
                    node(11, Some(10), "heading", "Caixa de entrada"),
                    10,
                    110,
                    300,
                    30,
                ),
                at(node(12, Some(10), "list", ""), 10, 150, 900, 600),
                at(
                    node(13, Some(12), "list item", "Contrato"),
                    10,
                    150,
                    900,
                    40,
                ),
                at(
                    node(14, Some(12), "list item", "Bem abaixo"),
                    10,
                    5000,
                    900,
                    40,
                ),
                at(
                    node(15, Some(10), "document web", "Iframe"),
                    20,
                    400,
                    500,
                    200,
                ),
                at(node(16, Some(15), "entry", "CEP"), 30, 420, 200, 30),
            ]),
            ViewOptions::default(),
        );
        assert_eq!(view.title.as_deref(), Some("Correio - Chromium"));
        assert_eq!(view.url.as_deref(), Some("example.com/mail"));
        assert_eq!(view.tabs, ["[5] page tab \"Correio\" {selected}"]);
        assert_eq!(
            view.outline(),
            "tab [5] page tab \"Correio\" {selected}\n\
             --- page ---\n\
             [10] document web \"Correio\" @0,100 1000x700\n  \
             [11] heading \"Caixa de entrada\" @10,110 300x30\n  \
             [12] list @10,150 900x600\n    \
             [13] list item \"Contrato\" @10,150 900x40\n  \
             [15] document web \"Iframe\" @20,400 500x200\n    \
             [16] entry \"CEP\" @30,420 200x30\n\
             (1 elements outside the visible window omitted; find_elements searches them)\n"
        );

        let everything = page_view(
            &browser(Vec::new()),
            ViewOptions {
                include_browser_ui: true,
            },
        );
        assert!(!everything.browser);
        assert!(everything.outline().contains("[4] button \"Reload\""));
        assert!(everything.outline().contains("[8] page tab \"English\""));
    }

    #[test]
    fn text_leaves_repeating_their_parent_line_are_folded() {
        let view = page_view(
            &browser(vec![
                with_text(
                    at(node(11, Some(10), "list item", ""), 0, 110, 900, 40),
                    "Marina CostaContrato de locação24 de out.",
                ),
                at(node(12, Some(11), "static", "Marina Costa"), 0, 110, 90, 20),
                at(
                    node(13, Some(11), "static", "Contrato de locação"),
                    100,
                    110,
                    90,
                    20,
                ),
                at(node(14, Some(11), "static", "Anexo"), 200, 110, 90, 20),
                with_text(
                    at(node(16, Some(11), "label", ""), 0, 130, 90, 20),
                    "24 de out.",
                ),
                at(node(17, Some(16), "static", "24 de out."), 0, 130, 90, 20),
                at(
                    node(15, Some(11), "button", "Com estrela"),
                    300,
                    110,
                    40,
                    40,
                ),
            ]),
            ViewOptions::default(),
        );
        let ids: Vec<u32> = view.entries.iter().map(|entry| entry.id).collect();
        assert_eq!(ids, [10, 11, 14, 15]);
        assert_eq!(view.omitted, 0);
    }

    #[test]
    fn visible_area_is_the_document_viewport_for_page_elements() {
        let nodes = browser(vec![at(
            node(11, Some(10), "button", "Ok"),
            10,
            5000,
            50,
            20,
        )]);
        let viewport = visible_area(&nodes, 11).map(|area| (area.y, area.height));
        assert_eq!(viewport, Some((100, 700)));
        // Browser UI is visible anywhere in the window.
        let frame = visible_area(&nodes, 4).map(|area| (area.y, area.height));
        assert_eq!(frame, Some((0, 800)));
    }

    #[test]
    fn a_tall_list_shows_only_its_part_inside_the_viewport() {
        let nodes = browser(vec![at(
            node(20, Some(10), "list", "Pedidos"),
            0,
            300,
            900,
            80_000,
        )]);
        let viewport = shown_part(&nodes, None).expect("the document viewport");
        let list = shown_part(&nodes, Some(20)).expect("the list's visible part");
        assert_eq!(list.y, 300);
        assert_eq!(list.y + list.height, viewport.y + viewport.height);
        assert!(shown_part(&nodes, Some(999)).is_none());
        let apart = Bounds {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let away = Bounds {
            x: 10,
            y: 0,
            width: 10,
            height: 10,
        };
        assert!(intersect(&apart, &away).is_none());
    }

    #[test]
    fn non_browser_views_draw_children_under_their_parent() {
        // Breadth-first snapshot order; the view is depth-first.
        let nodes = vec![
            node(1, None, "application", "gedit"),
            at(node(2, Some(1), "frame", "notes.txt"), 0, 0, 800, 600),
            node(3, Some(2), "panel", "Toolbar"),
            node(4, Some(2), "text", "Body"),
            node(5, Some(3), "push button", "Save"),
        ];
        let view = page_view(&nodes, ViewOptions::default());
        assert!(!view.browser);
        assert_eq!(view.title.as_deref(), Some("notes.txt"));
        assert_eq!(
            view.entries
                .iter()
                .map(|entry| (entry.id, entry.depth))
                .collect::<Vec<_>>(),
            [(1, 0), (2, 1), (3, 2), (5, 3), (4, 2)]
        );
    }

    #[test]
    fn change_report_lists_added_changed_and_removed_by_id() {
        let page = |reply_open: bool, subject: &str| {
            let mut page = vec![
                at(node(11, Some(10), "heading", subject), 10, 110, 300, 30),
                at(node(40, Some(10), "list item", "Marina"), 10, 150, 900, 40),
            ];
            if reply_open {
                page.pop();
                let mut body = at(node(88, Some(10), "entry", "Corpo"), 10, 400, 900, 160);
                body.states.push("focused".into());
                body.states.push("editable".into());
                page.push(body);
            }
            browser(page)
        };
        let before = page_view(&page(false, "Inbox"), ViewOptions::default());
        // A move alone is no change.
        let mut moved = page(false, "Inbox");
        if let Some(last) = moved.last_mut() {
            last.bounds = Some(Bounds {
                x: 10,
                y: 190,
                width: 900,
                height: 40,
            });
        }
        assert_eq!(
            change_report(&before, &page_view(&moved, ViewOptions::default())),
            "changes: none\n"
        );

        let mut after_nodes = page(true, "Contrato de locação");
        after_nodes[1] = at(
            node(2, Some(1), "frame", "Contrato - Chromium"),
            0,
            0,
            1000,
            800,
        );
        let after = page_view(&after_nodes, ViewOptions::default());
        assert_eq!(
            change_report(&before, &after),
            "changes (3):\n\
             + [88] entry \"Corpo\" @10,400 900x160 {focused,editable}\n\
             ~ [11] heading \"Contrato de locação\" @10,110 300x30\n\
             - [40] list item \"Marina\"\n\
             focus: [88] entry \"Corpo\" value=\"\"\n\
             title: \"Contrato - Chromium\"\n"
        );
    }

    #[test]
    fn a_closed_popup_is_one_removal() {
        let mut list = vec![at(node(20, Some(10), "list box", "Hora"), 0, 200, 100, 300)];
        list.extend((0..30).map(|row| {
            at(
                node(21 + row, Some(20), "list item", &format!("{row:02}:00")),
                0,
                200 + 10 * row as i32,
                100,
                10,
            )
        }));
        let before = page_view(&browser(list), ViewOptions::default());
        let after = page_view(&browser(Vec::new()), ViewOptions::default());
        assert_eq!(
            change_report(&before, &after),
            "changes (1):\n- [20] list box \"Hora\" (and 30 inside)\n"
        );
    }

    #[test]
    fn a_large_change_prints_the_whole_outline() {
        let before = page_view(&browser(Vec::new()), ViewOptions::default());
        let rows: Vec<AccessibilityNode> = (0..41)
            .map(|row| {
                at(
                    node(100 + row, Some(10), "row", &format!("r{row}")),
                    0,
                    100,
                    10,
                    10,
                )
            })
            .collect();
        let after = page_view(&browser(rows), ViewOptions::default());
        let report = change_report(&before, &after);
        assert!(
            report.starts_with("page changed (41 added, 0 removed); current outline:\n"),
            "{report}"
        );
        assert!(report.contains("--- page ---\n[10] document web"));
        assert!(report.contains("  [140] row \"r40\""));
        assert!(!report.contains("\n+ "));
    }

    fn disabled(mut node: AccessibilityNode) -> AccessibilityNode {
        node.states.retain(|state| state != "enabled");
        node
    }

    fn form(save: AccessibilityNode, extra: Vec<AccessibilityNode>) -> Vec<AccessibilityNode> {
        let mut nodes = vec![
            node(10, None, "document web", "Perfil"),
            node(11, Some(10), "entry", "Nome"),
            save,
        ];
        nodes.extend(extra);
        nodes
    }

    #[test]
    fn a_save_button_turning_into_salvando_is_busy_until_the_result() {
        let before = form(
            node(12, Some(10), "button", "Salvar alterações"),
            Vec::new(),
        );
        let saving = form(
            disabled(node(12, Some(10), "button", "Salvando…")),
            Vec::new(),
        );
        let busy = busy_reason(&before, &saving).expect("busy while saving");
        assert!(busy.strong);
        assert_eq!(busy.reason, "[12] button \"Salvando…\" shows a busy label");
        let saved = form(
            node(12, Some(10), "button", "Salvar alterações"),
            vec![node(13, Some(10), "status bar", "Alterações salvas")],
        );
        assert_eq!(busy_reason(&before, &saved), None);
        let failed = form(
            node(12, Some(10), "button", "Salvar alterações"),
            vec![node(
                13,
                Some(10),
                "notification",
                "Não foi possível salvar. Tente novamente.",
            )],
        );
        assert_eq!(busy_reason(&before, &failed), None);
    }

    #[test]
    fn busy_signs_are_states_progress_bars_words_and_ellipses() {
        let before = form(node(12, Some(10), "button", "Enviar"), Vec::new());
        let mut document = node(10, None, "document web", "Perfil");
        document.states.push("busy".into());
        let mut after = form(node(12, Some(10), "button", "Enviar"), Vec::new());
        after[0] = document;
        assert!(busy_reason(&before, &after).is_some_and(|busy| busy.reason.contains("is busy")));

        let bar = form(
            node(12, Some(10), "button", "Enviar"),
            vec![node(14, Some(10), "progress bar", "")],
        );
        assert!(busy_reason(&before, &bar).is_some_and(|busy| busy.reason.contains("appeared")));

        let loading = form(
            node(12, Some(10), "button", "Enviar"),
            vec![with_text(node(15, Some(10), "static", ""), "Please wait")],
        );
        assert!(busy_reason(&before, &loading).is_some_and(|busy| busy.strong));

        let dots = form(node(12, Some(10), "button", "Enviar..."), Vec::new());
        assert!(busy_reason(&before, &dots).is_some());
    }

    #[test]
    fn a_disabled_button_is_a_weak_sign_and_unchanged_pages_are_not_busy() {
        let before = form(node(12, Some(10), "button", "Enviar"), Vec::new());
        let after = form(disabled(node(12, Some(10), "button", "Enviar")), Vec::new());
        assert_eq!(
            busy_reason(&before, &after),
            Some(Busy {
                reason: "[12] button \"Enviar\" became disabled".into(),
                strong: false,
            })
        );
        // Already disabled, already busy-looking, or a new row that merely
        // ends in an ellipsis: nothing new to wait for.
        let still = form(disabled(node(12, Some(10), "button", "Enviar")), Vec::new());
        assert_eq!(busy_reason(&still, &still), None);
        let loading = form(node(12, Some(10), "button", "Carregando…"), Vec::new());
        assert_eq!(busy_reason(&loading, &loading), None);
        let row = form(
            node(12, Some(10), "button", "Enviar"),
            vec![node(16, Some(10), "row", "Pedido de cadeiras…")],
        );
        assert_eq!(busy_reason(&before, &row), None);
    }
}
