//! What the agent sees of a snapshot, and what changed between two of them.
//!
//! A browser window shows its title, URL and tabs, then only the visible web
//! document; other browser UI stays out of outlines and change reports but
//! remains in the snapshot, so `find_elements` and element ids still reach it.
//! Every view is drawn depth-first, children under their parent. Elements
//! outside the visible area are drawn too, nearest first, while the outline
//! stays within `OUTLINE_MAX_LINES` and `OUTLINE_MAX_BYTES`; their lines say
//! `(below)`, `(above)` or `(off-screen)` instead of a position.

use crate::atspi_tree::{AccessibilityNode, Bounds};
use crate::outline::{clean, element_label, render_line, render_line_placed, truncate};
use std::collections::{HashMap, HashSet};

/// Change lines above this many are cut to the `SHOWN_CHANGE_LINES` that
/// matter most, with a count of the rest.
pub const MAX_CHANGE_LINES: usize = 40;
/// Change lines a report over `MAX_CHANGE_LINES` prints.
pub const SHOWN_CHANGE_LINES: usize = 30;
/// Roles of the elements that tell the user something happened: alerts,
/// status messages (toasts, counters), dialogs. A change in or of one ranks
/// right after the acted element's own in a cut change report.
const NOTICE_ROLES: [&str; 6] = [
    "alert",
    "alert dialog",
    "dialog",
    "status bar",
    "notification",
    "log",
];
/// Most element lines a view draws once elements outside the visible area
/// count. Visible elements are always drawn; outside ones fill what is left.
pub const OUTLINE_MAX_LINES: usize = 250;
/// Most bytes of element lines (indentation and newline included) a view
/// draws once elements outside the visible area count.
pub const OUTLINE_MAX_BYTES: usize = 16 * 1024;

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
    /// `[id] role "name" @x,y wxh {states} ...`, unindented; `(below)`,
    /// `(above)` or `(off-screen)` replaces the position outside the
    /// visible area.
    pub line: String,
    /// `[id] role "name"`, for elements reported gone.
    pub label: String,
    /// Whether the element lies in the visible area (or has no bounds).
    pub visible: bool,
    /// Whether it is or lies inside an alert, status message or dialog
    /// (`NOTICE_ROLES`).
    notice: bool,
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
    /// Drawn elements, depth-first: every visible one, then as many outside
    /// the visible area as the outline budget allows.
    pub entries: Vec<ViewEntry>,
    /// Ids of elements outside the visible area left out for the budget.
    /// They still exist, so a change report does not call them gone.
    pub omitted: HashSet<u32>,
    /// The element holding keyboard focus, with its text when editable.
    pub focus: Option<String>,
}

impl PageView {
    /// Visible elements with names, text or actions: how much of the app
    /// the tree shows, which decides whether a screenshot adds anything.
    pub fn content_elements(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.visible)
            .filter(|entry| entry.line.contains('"') || entry.line.contains("actions="))
            .count()
    }

    /// The outline: tabs and a `--- page ---` rule for browsers, then one
    /// indented line per drawn element, then the omitted count.
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
        if !self.omitted.is_empty() {
            out.push_str(&format!(
                "({} elements outside the visible window omitted; find_elements searches them)\n",
                self.omitted.len()
            ));
        }
        out
    }
}

/// Builds the view of a snapshot whose `index`/`parent_index` are element
/// ids. With a web document present (and browser UI not requested), the view
/// is that document's subtree; otherwise the whole tree. Elements whose
/// bounds lie wholly outside the visible area (the document viewport, else
/// the window frame) are drawn with a marker for where they are, nearest
/// first, within the outline budget (`within_budget`); the rest are omitted
/// and counted.
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
    let candidates = view_candidates(&tree, roots, visible.as_ref());
    let costs: Vec<(Option<i64>, usize)> = candidates
        .iter()
        .map(|candidate| {
            let distance = candidate.outside.map(|(_, distance)| distance);
            (distance, candidate.line.len() + 2 * candidate.depth + 1)
        })
        .collect();
    let drawn = within_budget(&costs, OUTLINE_MAX_LINES, OUTLINE_MAX_BYTES);
    let ids: Vec<u32> = candidates
        .iter()
        .map(|candidate| nodes[candidate.at].index)
        .collect();

    let mut entries = Vec::new();
    let mut omitted = HashSet::new();
    // Per candidate: its depth when drawn, and its nearest drawn
    // ancestor-or-self. Parents come before their children.
    let mut drawn_depth = vec![0; candidates.len()];
    let mut anchor: Vec<Option<usize>> = vec![None; candidates.len()];
    for (at, candidate) in candidates.into_iter().enumerate() {
        let parent = candidate.parent.and_then(|parent| anchor[parent]);
        let node = &nodes[candidate.at];
        if !drawn[at] {
            anchor[at] = parent;
            omitted.insert(node.index);
            continue;
        }
        anchor[at] = Some(at);
        drawn_depth[at] = parent.map_or(0, |parent| drawn_depth[parent] + 1);
        entries.push(ViewEntry {
            id: node.index,
            parent: parent.map(|parent| ids[parent]),
            depth: drawn_depth[at],
            line: candidate.line,
            label: element_label(node),
            visible: candidate.outside.is_none(),
            notice: NOTICE_ROLES.contains(&tree.role(candidate.at).as_str())
                || tree
                    .ancestor_with_role(candidate.at, &NOTICE_ROLES)
                    .is_some(),
            key: content_key(node),
        });
    }

    PageView {
        title: frame.and_then(|frame| clean(nodes[frame].name.as_deref())),
        url: document.and_then(|doc| tree.address_bar(doc)),
        tabs: document.map(|doc| tree.page_tabs(doc)).unwrap_or_default(),
        browser: document.is_some(),
        entries,
        omitted,
        focus: tree.focused().map(|at| describe_focus(&nodes[at], None)),
    }
}

/// An element a view may draw, in depth-first order.
struct Candidate {
    /// Position in the snapshot.
    at: usize,
    /// Nearest ancestor that is a candidate too (its candidate position).
    parent: Option<usize>,
    /// Depth among candidates: the most indentation its line can get.
    depth: usize,
    /// Where it lies outside the visible area (`placement`), `None` inside.
    outside: Option<(&'static str, i64)>,
    line: String,
}

/// Which candidates a view draws, given each one's distance outside the
/// visible area (`None` inside) and the bytes its line costs: every inside
/// one, then outside ones nearest first (ties in document order) while the
/// drawn lines stay within `max_lines` and `max_bytes`. The first outside
/// one that does not fit ends the selection, so what is drawn is always the
/// nearest part.
fn within_budget(costs: &[(Option<i64>, usize)], max_lines: usize, max_bytes: usize) -> Vec<bool> {
    let mut drawn: Vec<bool> = costs
        .iter()
        .map(|(distance, _)| distance.is_none())
        .collect();
    let room = max_lines.saturating_sub(drawn.iter().filter(|&&drawn| drawn).count());
    let mut bytes: usize = costs
        .iter()
        .filter(|(distance, _)| distance.is_none())
        .map(|(_, cost)| cost)
        .sum();
    let mut outside: Vec<(i64, usize)> = costs
        .iter()
        .enumerate()
        .filter_map(|(at, (distance, _))| distance.map(|distance| (distance, at)))
        .collect();
    outside.sort_unstable();
    for (_, at) in outside.into_iter().take(room) {
        let cost = costs[at].1;
        if bytes + cost > max_bytes {
            break;
        }
        drawn[at] = true;
        bytes += cost;
    }
    drawn
}

/// Where an element lies when its bounds are wholly outside the visible
/// area: `below`, `above` or (beside it) `off-screen`, and how far from the
/// area's edge in pixels. `None` when it is (partly) inside or has no bounds.
fn placement(node: &AccessibilityNode, area: &Bounds) -> Option<(&'static str, i64)> {
    if !outside(node, area) {
        return None;
    }
    let bounds = node.bounds.as_ref()?;
    let (top, bottom) = (
        i64::from(bounds.y),
        i64::from(bounds.y) + i64::from(bounds.height),
    );
    let (left, right) = (
        i64::from(bounds.x),
        i64::from(bounds.x) + i64::from(bounds.width),
    );
    let (area_top, area_bottom) = (
        i64::from(area.y),
        i64::from(area.y) + i64::from(area.height),
    );
    let (area_left, area_right) = (i64::from(area.x), i64::from(area.x) + i64::from(area.width));
    let below = top >= area_bottom;
    let above = bottom <= area_top;
    let dy = if below {
        top - area_bottom
    } else if above {
        area_top - bottom
    } else {
        0
    };
    let dx = (left - area_right).max(area_left - right).max(0);
    let marker = if below {
        "below"
    } else if above {
        "above"
    } else {
        "off-screen"
    };
    Some((marker, dx.max(dy)))
}

/// Every element a view can draw, depth-first from `roots`: all but plain
/// text leaves whose words an ancestor's line already prints. A visible leaf
/// folds only into a visible ancestor, as when outside elements were never
/// drawn; an outside one into any ancestor.
fn view_candidates(tree: &Tree<'_>, roots: Vec<usize>, visible: Option<&Bounds>) -> Vec<Candidate> {
    let nodes = tree.nodes;
    let mut candidates = Vec::new();
    // (position, parent candidate, depth, nearest visible candidate ancestor
    // carrying text, nearest candidate ancestor carrying text)
    type Pending = (usize, Option<usize>, usize, Option<usize>, Option<usize>);
    let mut stack: Vec<Pending> = roots
        .into_iter()
        .rev()
        .map(|at| (at, None, 0, None, None))
        .collect();
    let mut visited = vec![false; nodes.len()];
    while let Some((at, parent, depth, visible_context, any_context)) = stack.pop() {
        if std::mem::replace(&mut visited[at], true) {
            continue;
        }
        let node = &nodes[at];
        let outside = visible.and_then(|area| placement(node, area));
        let context = if outside.is_none() {
            visible_context
        } else {
            any_context
        };
        if tree.text_only(at)
            && context.is_some_and(|context| repeats_text_of(node, &nodes[context]))
        {
            // A text leaf whose words its ancestor's line already shows.
            continue;
        }
        let index = candidates.len();
        candidates.push(Candidate {
            at,
            parent,
            depth,
            outside,
            line: render_line_placed(node, outside.map(|(marker, _)| marker)),
        });
        let carries_text = node_text(node).is_some() || clean(node.name.as_deref()).is_some();
        let child_visible_context = if carries_text && outside.is_none() {
            Some(at)
        } else {
            visible_context
        };
        let child_any_context = if carries_text { Some(at) } else { any_context };
        for &child in tree.children(at).iter().rev() {
            stack.push((
                child,
                Some(index),
                depth + 1,
                child_visible_context,
                child_any_context,
            ));
        }
    }
    candidates
}
/// What a settle read refreshes of a browser snapshot instead of the whole
/// app: the visible web document, read again with its subtree; the tab
/// strip holding the browser tabs, read again whole, so a tab opened in the
/// background shows; and the window frame and address bar, read again on
/// their own for the title and URL. The rest of the browser UI is kept from
/// the earlier read (`splice_document`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentParts {
    /// Id of the visible web document.
    pub document: u32,
    /// Ids of the browser tabs, in snapshot order; the settle read rereads
    /// the element holding them.
    pub tabs: Vec<u32>,
    /// Id of its window frame, whose name is the window title.
    pub frame: Option<u32>,
    /// Id of the address bar entry.
    pub address_bar: Option<u32>,
}

/// The `DocumentParts` of a snapshot with stable ids; `None` without a
/// visible web document.
pub fn document_parts(nodes: &[AccessibilityNode]) -> Option<DocumentParts> {
    let tree = Tree::new(nodes);
    let document = tree.visible_document()?;
    Some(DocumentParts {
        document: nodes[document].index,
        tabs: tree
            .browser_tabs(document)
            .into_iter()
            .map(|at| nodes[at].index)
            .collect(),
        frame: tree
            .ancestor_with_role(document, &["frame", "window", "dialog"])
            .map(|at| nodes[at].index),
        address_bar: tree.address_bar_entry(document).map(|at| nodes[at].index),
    })
}

/// A settle read of a snapshot's `DocumentParts`, with stable ids.
#[derive(Debug, Clone, Default)]
pub struct FreshParts {
    /// The document's subtree, the document first.
    pub document: Vec<AccessibilityNode>,
    /// The element holding the browser tabs, with everything in it (its
    /// first nodes may lack a parent when compaction dropped that element);
    /// empty when the snapshot shows no tabs.
    pub tab_strip: Vec<AccessibilityNode>,
    /// The frame and address bar, each read on its own (`index` set to its
    /// id).
    pub singles: Vec<AccessibilityNode>,
}

/// `base` with the settle read `fresh` of its `parts` in place: the
/// document's subtree replaced by the fresh one, the tabs (with what they
/// hold, and every element the tab strip read holds) replaced by the fresh
/// tab strip, each placed where the old one was, its parentless nodes under
/// the old one's parent; and each single in place of the base element with
/// its id, keeping its place in the tree. Focus comes from the fresh parts
/// only: a kept element loses a `focused` state unless it is an ancestor of
/// the fresh focus. `None` when `base` lacks an element of `parts`, the tab
/// strip read reaches into another part, or no fresh part holds focus
/// (focus may have moved to browser UI kept from the earlier read).
pub fn splice_document(
    base: &[AccessibilityNode],
    parts: &DocumentParts,
    fresh: FreshParts,
) -> Option<Vec<AccessibilityNode>> {
    let tree = Tree::new(base);
    let position = |id: u32| base.iter().position(|node| node.index == id);
    let document = position(parts.document)?;
    let tabs: Vec<usize> = parts
        .tabs
        .iter()
        .map(|&id| position(id))
        .collect::<Option<_>>()?;
    let strip_ids: HashSet<u32> = fresh.tab_strip.iter().map(|node| node.index).collect();
    let other_parts = [Some(parts.document), parts.frame, parts.address_bar];
    if other_parts
        .into_iter()
        .flatten()
        .any(|id| strip_ids.contains(&id))
    {
        return None;
    }
    let in_strip = |at: usize| {
        strip_ids.contains(&base[at].index) || tabs.iter().any(|&tab| tree.inside(at, tab))
    };
    let mut document_nodes = Some(fresh.document);
    let mut strip_nodes = Some(fresh.tab_strip);
    let mut spliced = Vec::with_capacity(base.len());
    let mut fresh_at = HashSet::new();
    let mut graft = |spliced: &mut Vec<AccessibilityNode>,
                     nodes: Vec<AccessibilityNode>,
                     parent: Option<u32>| {
        for mut node in nodes {
            if node.parent_index.is_none() {
                node.parent_index = parent;
            }
            fresh_at.insert(spliced.len());
            spliced.push(node);
        }
    };
    for (at, node) in base.iter().enumerate() {
        if at == document {
            let nodes = document_nodes.take().unwrap_or_default();
            graft(&mut spliced, nodes, node.parent_index);
        } else if tree.inside(at, document) {
            continue;
        } else if in_strip(at) {
            if let Some(nodes) = strip_nodes.take() {
                graft(&mut spliced, nodes, node.parent_index);
            }
        } else if let Some(new) = fresh.singles.iter().find(|new| new.index == node.index) {
            graft(
                &mut spliced,
                vec![AccessibilityNode {
                    parent_index: node.parent_index,
                    depth: node.depth,
                    child_count: node.child_count,
                    ..new.clone()
                }],
                None,
            );
        } else {
            spliced.push(node.clone());
        }
    }
    let view = Tree::new(&spliced);
    let focus = fresh_at
        .iter()
        .copied()
        .filter(|&at| has_state(&spliced[at], "focused"))
        .max_by_key(|&at| view.ancestors(at).count())?;
    let holders: HashSet<usize> = view.ancestors(focus).collect();
    for (at, node) in spliced.iter_mut().enumerate() {
        if !fresh_at.contains(&at) && !holders.contains(&at) {
            node.states
                .retain(|state| !state.eq_ignore_ascii_case("focused"));
        }
    }
    Some(spliced)
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

/// Roles of containers whose rows a page may render only near the viewport.
const LIST_ROLES: [&str; 7] = [
    "list",
    "list box",
    "table",
    "tree",
    "tree table",
    "grid",
    "feed",
];

/// Ids of the lists, tables and trees that hold rows and reach past the
/// area where they can be seen (`visible_area`): scrolling one can render
/// rows the tree lacks now, as a virtualized list does. In tree order; a
/// list inside another one listed is left out.
pub fn scrollable_lists(nodes: &[AccessibilityNode]) -> Vec<u32> {
    let tree = Tree::new(nodes);
    let mut lists: Vec<usize> = Vec::new();
    for at in 0..nodes.len() {
        let node = &nodes[at];
        if !LIST_ROLES.contains(&tree.role(at).as_str())
            || tree.children(at).is_empty()
            || lists.iter().any(|&list| tree.inside(at, list))
        {
            continue;
        }
        let Some(bounds) = &node.bounds else {
            continue;
        };
        let Some(area) = visible_area(nodes, node.index) else {
            continue;
        };
        let reaches_past = bounds.y < area.y
            || bounds.x < area.x
            || bounds.y + bounds.height > area.y + area.height
            || bounds.x + bounds.width > area.x + area.width;
        if reaches_past {
            lists.push(at);
        }
    }
    lists.into_iter().map(|at| nodes[at].index).collect()
}

/// An element's line placed against a snapshot of its page: `(below)`,
/// `(above)` or `(off-screen)` in place of its position when it lies wholly
/// outside where it can be seen (`visible_area`; for an element the
/// snapshot lacks, the page's area: `shown_part`).
pub fn placed_line(nodes: &[AccessibilityNode], node: &AccessibilityNode) -> String {
    let area = if nodes.iter().any(|candidate| candidate.index == node.index) {
        visible_area(nodes, node.index)
    } else {
        shown_part(nodes, None)
    };
    let marker = area
        .and_then(|area| placement(node, &area))
        .map(|(marker, _)| marker);
    render_line_placed(node, marker)
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
/// new elements, `-` gone ones, `~` same id with different content or newly
/// in the visible area (other moves do not count), then the focus, and the
/// title and URL when they changed, and the browser tabs when one opened or
/// closed. An element the current view leaves out
/// for its budget is not gone. Over `MAX_CHANGE_LINES` lines only the
/// `SHOWN_CHANGE_LINES` that matter most are printed (`ChangeRank`: the
/// elements acted on, `targets`, first), in their usual order, and the rest
/// are counted.
pub fn change_report(before: &PageView, after: &PageView, targets: &[u32]) -> String {
    let before_keys: HashMap<u32, &ViewEntry> = before
        .entries
        .iter()
        .map(|entry| (entry.id, entry))
        .collect();
    let after_ids: HashSet<u32> = after
        .entries
        .iter()
        .map(|entry| entry.id)
        .chain(after.omitted.iter().copied())
        .collect();
    let rank = |entry: &ViewEntry, new: bool| {
        if targets.contains(&entry.id) {
            ChangeRank::Target
        } else if new && entry.notice {
            ChangeRank::Notice
        } else if entry.visible {
            ChangeRank::Visible
        } else {
            ChangeRank::Other
        }
    };
    let mut added = Vec::new();
    let mut changed = Vec::new();
    for entry in &after.entries {
        match before_keys.get(&entry.id) {
            None => added.push((format!("+ {}", entry.line), rank(entry, true))),
            Some(old) if old.key != entry.key || (entry.visible && !old.visible) => {
                changed.push((format!("~ {}", entry.line), rank(entry, true)))
            }
            Some(_) => {}
        }
    }
    // A removed container stands for everything inside it.
    let gone: HashSet<u32> = before
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
    let removed: Vec<(String, ChangeRank)> = removed
        .into_iter()
        .map(|entry| {
            let line = match inside_gone.get(&entry.id) {
                Some(count) => format!("- {} (and {count} inside)", entry.label),
                None => format!("- {}", entry.label),
            };
            (line, rank(entry, false))
        })
        .collect();

    let mut out = String::new();
    let lines: Vec<&(String, ChangeRank)> = added.iter().chain(&changed).chain(&removed).collect();
    let total = lines.len();
    if total == 0 {
        out.push_str("changes: none\n");
    } else if total > MAX_CHANGE_LINES {
        let mut by_rank: Vec<usize> = (0..total).collect();
        by_rank.sort_by_key(|&at| (lines[at].1, at));
        let shown: HashSet<usize> = by_rank.into_iter().take(SHOWN_CHANGE_LINES).collect();
        out.push_str(&format!(
            "changes ({total}; the {SHOWN_CHANGE_LINES} that matter most):\n"
        ));
        for (at, (line, _)) in lines.iter().enumerate() {
            if shown.contains(&at) {
                out.push_str(line);
                out.push('\n');
            }
        }
        let hidden = |range: std::ops::Range<usize>| range.filter(|at| !shown.contains(at)).count();
        let counts: Vec<String> = [
            (hidden(0..added.len()), "added"),
            (hidden(added.len()..added.len() + changed.len()), "changed"),
            (hidden(added.len() + changed.len()..total), "removed"),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, kind)| format!("{count} {kind}"))
        .collect();
        out.push_str(&format!(
            "… and {} more changes ({}); call get_app_state for the whole page\n",
            total - SHOWN_CHANGE_LINES,
            counts.join(", ")
        ));
    } else {
        out.push_str(&format!("changes ({total}):\n"));
        for (line, _) in lines {
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
    // Tab lines start with the tab's id.
    let tab_ids = |view: &PageView| -> Vec<String> {
        view.tabs
            .iter()
            .filter_map(|tab| tab.split(' ').next().map(str::to_string))
            .collect()
    };
    if tab_ids(after) != tab_ids(before) {
        out.push_str(&format!("tabs: {}\n", after.tabs.join("; ")));
    }
    out
}

/// How much a change line matters when a report must leave some out, most
/// first: the element acted on, a new or changed alert, status message or
/// dialog (`NOTICE_ROLES`), anything in the visible area, the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ChangeRank {
    Target,
    Notice,
    Visible,
    Other,
}

/// Roles of a dialog that can stand in front of the rest of a page.
const DIALOG_ROLES: [&str; 3] = ["dialog", "alert dialog", "alert"];
/// Most buttons a blocking-dialog note lists.
const DIALOG_BUTTONS_SHOWN: usize = 8;

/// A dialog in `after` that is likely in the way of an action, as a note:
/// `a dialog is open: [id] dialog "name" — buttons: [id] "label", …`.
/// It must be shown, inside the visible web document when there is one
/// (browser bubbles do not count), and modal (`modal` state, or an alert
/// dialog) or new since `before` (elements matched by id). A plain `alert`
/// counts only with a button, so status messages are not taken for dialogs.
/// Modal ones win over new ones; among equals the last in the tree, which
/// pages usually add on top.
pub fn blocking_dialog(
    before: &[AccessibilityNode],
    after: &[AccessibilityNode],
) -> Option<String> {
    let tree = Tree::new(after);
    let document = tree.visible_document();
    let earlier: std::collections::HashSet<u32> = before.iter().map(|node| node.index).collect();
    let buttons = |dialog: usize| -> Vec<usize> {
        (0..after.len())
            .filter(|&at| at != dialog && tree.inside(at, dialog))
            .filter(|&at| tree.role(at).ends_with("button"))
            .filter(|&at| shown_label(&after[at]).is_some())
            .collect()
    };
    let candidates: Vec<(usize, bool)> = (0..after.len())
        .filter(|&at| DIALOG_ROLES.contains(&tree.role(at).as_str()))
        .filter(|&at| has_state(&after[at], "showing") || after[at].bounds.is_some())
        .filter(|&at| document.is_none_or(|document| at != document && tree.inside(at, document)))
        .filter(|&at| tree.role(at) != "alert" || !buttons(at).is_empty())
        .filter_map(|at| {
            let modal = has_state(&after[at], "modal") || tree.role(at) == "alert dialog";
            (modal || !earlier.contains(&after[at].index)).then_some((at, modal))
        })
        .collect();
    let (dialog, _) = candidates
        .iter()
        .rev()
        .find(|(_, modal)| *modal)
        .or_else(|| candidates.last())
        .copied()?;
    let node = &after[dialog];
    let name = clean(node.name.as_deref()).or_else(|| {
        (0..after.len())
            .filter(|&at| tree.role(at) == "heading" && tree.inside(at, dialog))
            .find_map(|at| shown_label(&after[at]))
    });
    let mut note = format!("a dialog is open: [{}] {}", node.index, node.role);
    if let Some(name) = name {
        note.push_str(&format!(" \"{}\"", truncate(&name, 80)));
    }
    let buttons = buttons(dialog);
    if !buttons.is_empty() {
        let listed: Vec<String> = buttons
            .iter()
            .take(DIALOG_BUTTONS_SHOWN)
            .filter_map(|&at| {
                shown_label(&after[at])
                    .map(|label| format!("[{}] \"{}\"", after[at].index, truncate(&label, 40)))
            })
            .collect();
        note.push_str(" — buttons: ");
        note.push_str(&listed.join(", "));
        if buttons.len() > DIALOG_BUTTONS_SHOWN {
            note.push_str(&format!(
                ", … ({} more)",
                buttons.len() - DIALOG_BUTTONS_SHOWN
            ));
        }
    }
    Some(note)
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
        let label = shown_label(node);
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
                shown_label(old).as_deref() != Some(label)
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

/// The name, else the text, an element shows: what a busy check reads and
/// what a dialog note calls a button or heading.
fn shown_label(node: &AccessibilityNode) -> Option<String> {
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

    /// The address bar text (`address_bar_entry`).
    fn address_bar(&self, document: usize) -> Option<String> {
        self.address_bar_entry(document)
            .and_then(|at| node_text(&self.nodes[at]))
    }

    /// The address bar: an entry outside the document named like an address
    /// bar and holding text, else one whose text looks like a URL.
    fn address_bar_entry(&self, document: usize) -> Option<usize> {
        let entries: Vec<usize> = (0..self.nodes.len())
            .filter(|&at| self.role(at) == "entry" && !self.inside(at, document))
            .collect();
        let text = |at: usize| node_text(&self.nodes[at]);
        entries
            .iter()
            .copied()
            .find(|&at| {
                self.nodes[at]
                    .name
                    .as_deref()
                    .is_some_and(|name| name.to_lowercase().contains("address"))
            })
            .filter(|&at| text(at).is_some())
            .or_else(|| {
                entries.iter().copied().find(|&at| {
                    text(at)
                        .is_some_and(|text| !text.contains(' ') && text.contains(['.', '/', ':']))
                })
            })
    }

    /// Browser tabs: page tabs outside the document that do not belong to a
    /// bubble or dialog (Chromium's translate bubble has page tabs too).
    fn page_tabs(&self, document: usize) -> Vec<String> {
        self.browser_tabs(document)
            .into_iter()
            .map(|at| render_line(&self.nodes[at]))
            .collect()
    }

    /// Positions of the browser tabs (`page_tabs`).
    fn browser_tabs(&self, document: usize) -> Vec<usize> {
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

/// The element holding keyboard focus in a snapshot: the deepest one
/// reporting `focused`.
pub fn focused_element(nodes: &[AccessibilityNode]) -> Option<&AccessibilityNode> {
    Tree::new(nodes).focused().map(|at| &nodes[at])
}

/// Whether the focus line needs a fuller read of the element for its
/// value: a rich-text editor whose own text is only the embedded-object
/// marks of its paragraphs (a Chromium contenteditable).
pub fn focus_needs_full_text(node: &AccessibilityNode) -> bool {
    crate::fields::is_editable(node)
        && node
            .text
            .as_ref()
            .and_then(|text| text.content.as_deref())
            .is_some_and(|text| text.contains(crate::fields::EMBEDDED_OBJECT))
}

/// `[id] role "name"`, plus `value="…"` for an editable element: `full_text`
/// when the caller read the editor with its paragraphs
/// (`fields::expanded_text`), else the element's own text. A multi-line
/// value prints its lines joined by a literal `\n`, as set_value reads it
/// back. A rich-text editor's own text is only embedded-object marks
/// (`focus_needs_full_text`), so without `full_text` its value is unknown
/// and left out rather than shown as `""`.
pub fn describe_focus(node: &AccessibilityNode, full_text: Option<&str>) -> String {
    let mut line = element_label(node);
    if !crate::fields::is_editable(node) {
        return line;
    }
    let value = match full_text {
        Some(text) => Some(crate::fields::shown_text(Some(text))),
        None if focus_needs_full_text(node) => None,
        None => Some(
            crate::fields::field_text(node)
                .map(|text| crate::fields::shown_text(Some(&text)))
                .unwrap_or_default(),
        ),
    };
    if let Some(value) = value {
        line.push_str(&format!(" value=\"{}\"", truncate(&value, 200)));
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

    /// A contact form, and (when `dialog`) the newsletter dialog that opens
    /// over it with the given extra states.
    fn contact_page(dialog: Option<&[&str]>) -> Vec<AccessibilityNode> {
        let mut page = vec![
            node(20, Some(10), "form", "Contato"),
            node(21, Some(20), "entry", "Nome"),
            node(22, Some(20), "push button", "Enviar mensagem"),
        ];
        if let Some(states) = dialog {
            let mut dialog = node(30, Some(10), "dialog", "");
            dialog
                .states
                .extend(states.iter().map(|state| state.to_string()));
            page.extend([
                dialog,
                node(31, Some(30), "heading", "Receba nossas novidades"),
                node(32, Some(30), "entry", "E-mail"),
                node(33, Some(30), "push button", "Agora não"),
                node(34, Some(30), "push button", "Assinar"),
                node(35, Some(30), "push button", "Fechar"),
            ]);
        }
        browser(page)
    }

    #[test]
    fn a_modal_or_new_dialog_in_the_page_is_named_with_its_buttons() {
        let before = contact_page(None);
        let expected = "a dialog is open: [30] dialog \"Receba nossas novidades\" — buttons: [33] \"Agora não\", [34] \"Assinar\", [35] \"Fechar\"";
        // New since the snapshot the agent saw.
        assert_eq!(
            blocking_dialog(&before, &contact_page(Some(&[]))).as_deref(),
            Some(expected)
        );
        // Already known but modal.
        let modal = contact_page(Some(&["modal"]));
        assert_eq!(blocking_dialog(&modal, &modal).as_deref(), Some(expected));
        // Already known and not modal: not in the way.
        let open = contact_page(Some(&[]));
        assert_eq!(blocking_dialog(&open, &open), None);
        // No dialog at all; the browser's translate bubble (an alert outside
        // the page) does not count either.
        assert_eq!(blocking_dialog(&[], &before), None);
        // A closed dialog left in the tree is not shown.
        let mut closed = contact_page(Some(&["modal"]));
        for node in &mut closed {
            if node.index == 30 {
                node.states.retain(|state| state != "showing");
            }
        }
        assert_eq!(blocking_dialog(&closed, &closed), None);
        // A new status alert without buttons is not a dialog.
        let mut status = before.clone();
        status.push(node(40, Some(10), "alert", "Mensagem enviada"));
        assert_eq!(blocking_dialog(&before, &status), None);
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
             [13] list item \"Contrato\" @10,150 900x40\n    \
             [14] list item \"Bem abaixo\" (below)\n  \
             [15] document web \"Iframe\" @20,400 500x200\n    \
             [16] entry \"CEP\" @30,420 200x30\n"
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
        assert!(view.omitted.is_empty());
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

    fn focused(mut node: AccessibilityNode) -> AccessibilityNode {
        node.states.push("focused".into());
        node
    }

    #[test]
    fn a_settle_read_splices_the_fresh_document_into_the_browser_snapshot() {
        let mut base = browser(vec![
            at(
                node(11, Some(10), "heading", "Caixa de entrada"),
                10,
                110,
                300,
                30,
            ),
            at(node(12, Some(10), "button", "Enviar"), 10, 150, 90, 30),
            at(node(13, Some(12), "static", "Enviar"), 10, 150, 90, 30),
        ]);
        base[8].states.push("focused".into());
        let parts = document_parts(&base).expect("a visible document");
        assert_eq!(
            parts,
            DocumentParts {
                document: 10,
                tabs: vec![5],
                frame: Some(2),
                address_bar: Some(3),
            }
        );
        assert_eq!(document_parts(&base[..8]), None);

        // The fresh read: the button is gone, a status appeared; the frame and
        // address bar were read again on their own (no parent or depth).
        let mut document = at(node(10, None, "document web", "Correio"), 0, 100, 1000, 700);
        document.depth = 2;
        let document = focused(document);
        let fresh = vec![
            document,
            at(
                node(11, Some(10), "heading", "Caixa de entrada"),
                10,
                110,
                300,
                30,
            ),
            at(
                node(14, Some(10), "status bar", "Mensagem enviada"),
                10,
                700,
                300,
                30,
            ),
        ];
        let frame = node(2, None, "frame", "Enviado - Chromium");
        let address = with_text(
            node(3, None, "entry", "Address and search bar"),
            "example.com/sent",
        );
        let mut tab = node(5, None, "page tab", "Correio");
        tab.states.push("selected".into());
        let fresh = FreshParts {
            document: fresh,
            tab_strip: vec![tab],
            singles: vec![frame, address],
        };
        let spliced = splice_document(&base, &parts, fresh.clone()).expect("spliced");

        let ids: Vec<u32> = spliced.iter().map(|node| node.index).collect();
        assert_eq!(ids, [1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 14]);
        assert_eq!(spliced[1].parent_index, Some(1));
        assert_eq!(spliced[4].parent_index, Some(2));
        assert_eq!(spliced[8].parent_index, Some(2));
        let before = page_view(&base, ViewOptions::default());
        let after = page_view(&spliced, ViewOptions::default());
        assert_eq!(after.tabs, before.tabs);
        assert_eq!(
            change_report(&before, &after, &[]),
            "changes (2):\n\
             + [14] status bar \"Mensagem enviada\" @10,700 300x30\n\
             - [12] button \"Enviar\"\n\
             focus: [10] document web \"Correio\"\n\
             title: \"Enviado - Chromium\"\n\
             url: example.com/sent\n"
        );
        let mut elsewhere = parts.clone();
        elsewhere.document = 99;
        assert!(splice_document(&base, &elsewhere, fresh).is_none());
    }

    #[test]
    fn a_settle_read_shows_a_tab_opened_in_the_background() {
        let base = browser(vec![focused(at(
            node(12, Some(10), "link", "Ver boleto"),
            10,
            150,
            90,
            30,
        ))]);
        let parts = document_parts(&base).expect("a visible document");
        let mut document = at(node(10, None, "document web", "Correio"), 0, 100, 1000, 700);
        document.depth = 2;
        let page = vec![
            document,
            focused(at(
                node(12, Some(10), "link", "Ver boleto"),
                10,
                150,
                90,
                30,
            )),
        ];
        // The tab strip read again: the link opened a tab behind this one.
        // Its own element was compacted away, so the tabs have no parent.
        let mut tab = node(5, None, "page tab", "Correio");
        tab.states.push("selected".into());
        let fresh = FreshParts {
            document: page.clone(),
            tab_strip: vec![tab, node(40, None, "page tab", "Boleto 2026-10")],
            singles: vec![node(2, None, "frame", "Correio - Chromium")],
        };
        let spliced = splice_document(&base, &parts, fresh).expect("spliced");
        let before = page_view(&base, ViewOptions::default());
        let after = page_view(&spliced, ViewOptions::default());
        assert_eq!(after.tabs.len(), 2, "{:?}", after.tabs);
        assert!(after.tabs[1].contains("Boleto 2026-10"), "{:?}", after.tabs);
        assert_eq!(
            spliced
                .iter()
                .find(|node| node.index == 40)
                .and_then(|node| node.parent_index),
            Some(2)
        );
        assert_eq!(
            change_report(&before, &after, &[]),
            "changes: none\n\
             focus: [12] link \"Ver boleto\"\n\
             tabs: [5] page tab \"Correio\" {selected}; [40] page tab \"Boleto 2026-10\"\n"
        );

        // A closed tab leaves the strip.
        let fresh = FreshParts {
            document: page,
            tab_strip: Vec::new(),
            singles: Vec::new(),
        };
        let spliced = splice_document(&base, &parts, fresh).expect("spliced");
        assert!(page_view(&spliced, ViewOptions::default()).tabs.is_empty());
    }

    #[test]
    fn a_settle_read_takes_focus_from_the_fresh_parts_only() {
        // The address bar held focus; a click in the page moved it to the
        // link. The reload button kept a stale focused state.
        let mut base = browser(vec![at(
            node(12, Some(10), "link", "Ver boleto"),
            10,
            150,
            90,
            30,
        )]);
        base[3].states.push("focused".into());
        let parts = document_parts(&base).expect("a visible document");
        let mut document = at(node(10, None, "document web", "Correio"), 0, 100, 1000, 700);
        document.depth = 2;
        let page = vec![
            document,
            focused(at(
                node(12, Some(10), "link", "Ver boleto"),
                10,
                150,
                90,
                30,
            )),
        ];
        let mut tab = node(5, None, "page tab", "Correio");
        tab.states.push("selected".into());
        let fresh = FreshParts {
            document: page.clone(),
            tab_strip: vec![tab.clone()],
            singles: Vec::new(),
        };
        let spliced = splice_document(&base, &parts, fresh).expect("spliced");
        let still_focused: Vec<u32> = spliced
            .iter()
            .filter(|node| has_state(node, "focused"))
            .map(|node| node.index)
            .collect();
        assert_eq!(still_focused, [12]);

        // No fresh part holds focus: it may be in browser UI the read kept,
        // so the read cannot stand in for the whole scope.
        let mut unfocused = page;
        unfocused[1].states.clear();
        let fresh = FreshParts {
            document: unfocused,
            tab_strip: vec![tab],
            singles: Vec::new(),
        };
        assert!(splice_document(&base, &parts, fresh).is_none());
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
            change_report(&before, &page_view(&moved, ViewOptions::default()), &[]),
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
            change_report(&before, &after, &[]),
            "changes (3):\n\
             + [88] entry \"Corpo\" @10,400 900x160 {focused,editable}\n\
             ~ [11] heading \"Contrato de locação\" @10,110 300x30\n\
             - [40] list item \"Marina\"\n\
             focus: [88] entry \"Corpo\" value=\"\"\n\
             title: \"Contrato - Chromium\"\n"
        );
    }

    #[test]
    fn the_focus_line_never_shows_a_rich_text_editor_as_empty() {
        let mut editor = with_text(
            node(88, Some(10), "entry", "Corpo da mensagem"),
            "\u{fffc}\u{fffc}",
        );
        editor.states = vec!["focused".into(), "editable".into(), "multi-line".into()];
        assert!(focus_needs_full_text(&editor));
        // Its own text is only paragraph marks: the value is unknown.
        assert_eq!(
            describe_focus(&editor, None),
            "[88] entry \"Corpo da mensagem\""
        );
        assert_eq!(
            describe_focus(&editor, Some("Olá Ana,\nSegue o contrato.\n")),
            "[88] entry \"Corpo da mensagem\" value=\"Olá Ana,\\nSegue o contrato.\""
        );
        let view = page_view(&browser(vec![editor]), ViewOptions::default());
        assert_eq!(
            view.focus.as_deref(),
            Some("[88] entry \"Corpo da mensagem\"")
        );

        // A text area's own text is its value, lines and all; an empty
        // field is still shown empty.
        let mut area = with_text(node(89, Some(10), "entry", "Notas"), "um\ndois");
        area.states = vec!["editable".into(), "multi-line".into()];
        assert!(!focus_needs_full_text(&area));
        assert_eq!(
            describe_focus(&area, None),
            "[89] entry \"Notas\" value=\"um\\ndois\""
        );
        let mut empty = node(90, Some(10), "entry", "Assunto");
        empty.states = vec!["editable".into()];
        assert_eq!(
            describe_focus(&empty, None),
            "[90] entry \"Assunto\" value=\"\""
        );
    }

    #[test]
    fn a_target_line_says_where_the_element_is_and_its_state() {
        let page = inbox(30, 110);
        // A row read again after a click: checked, still below the fold.
        let mut row = page
            .iter()
            .find(|node| node.index == 125)
            .cloned()
            .expect("row 25 is in the inbox");
        row.role = "check box".into();
        row.states = vec!["checked".into(), "enabled".into()];
        assert_eq!(
            placed_line(&page, &row),
            "[125] check box \"Mensagem 025\" (below) {checked}"
        );
        // Inside the viewport it keeps its position.
        let visible = page
            .iter()
            .find(|node| node.index == 101)
            .cloned()
            .expect("row 1 is in the inbox");
        assert_eq!(
            placed_line(&page, &visible),
            "[101] list item \"Mensagem 001\" @0,150 1000x40"
        );
        // An element the snapshot lacks is placed against the page.
        let mut popup = at(node(900, None, "button", "Fechar"), 0, 2000, 50, 20);
        popup.states = vec!["enabled".into()];
        assert_eq!(
            placed_line(&page, &popup),
            "[900] button \"Fechar\" (below)"
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
            change_report(&before, &after, &[]),
            "changes (1):\n- [20] list box \"Hora\" (and 30 inside)\n"
        );
    }

    #[test]
    fn a_large_change_prints_the_acted_element_notices_and_visible_lines_first() {
        let check_box = |checked: bool| {
            let mut node = at(
                node(60, Some(10), "check box", "Selecionar"),
                0,
                2000,
                20,
                20,
            );
            if checked {
                node.states = vec!["checked".into()];
            }
            node
        };
        let before = page_view(&browser(vec![check_box(false)]), ViewOptions::default());
        let mut page: Vec<AccessibilityNode> = (0..41)
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
        page.push(at(
            node(200, Some(10), "status bar", "Pedido arquivado"),
            0,
            700,
            300,
            20,
        ));
        page.push(check_box(true));
        let after = page_view(&browser(page), ViewOptions::default());
        let report = change_report(&before, &after, &[60]);
        let lines: Vec<&str> = report.lines().collect();
        assert_eq!(lines[0], "changes (43; the 30 that matter most):");
        // In their usual order: added, then changed.
        assert_eq!(lines[1], "+ [100] row \"r0\" @0,100 10x10");
        assert_eq!(lines[28], "+ [127] row \"r27\" @0,100 10x10");
        assert_eq!(
            lines[29],
            "+ [200] status bar \"Pedido arquivado\" @0,700 300x20"
        );
        assert_eq!(
            lines[30],
            "~ [60] check box \"Selecionar\" (below) {checked}"
        );
        assert_eq!(
            lines[31],
            "… and 13 more changes (13 added); call get_app_state for the whole page"
        );
        assert!(!report.contains("[128] row"), "{report}");
        assert!(!report.contains("--- page ---"), "{report}");
    }

    /// A mail list of `rows` rows 40 px tall from y=110, in a 700 px tall
    /// viewport starting at y=100: from `first_y` 110, rows 0-17 are
    /// visible and the rest below.
    fn inbox(rows: u32, first_y: i32) -> Vec<AccessibilityNode> {
        let mut page = vec![at(
            node(11, Some(10), "list", "Caixa de entrada"),
            0,
            first_y,
            1000,
            40 * rows as i32,
        )];
        page.extend((0..rows).map(|row| {
            at(
                node(
                    100 + row,
                    Some(11),
                    "list item",
                    &format!("Mensagem {row:03}"),
                ),
                0,
                first_y + 40 * row as i32,
                1000,
                40,
            )
        }));
        browser(page)
    }

    #[test]
    fn lists_reaching_past_the_viewport_are_the_ones_scrolling_can_fill() {
        // 30 rows of 40 px from y=110 run past the 700 px viewport.
        assert_eq!(scrollable_lists(&inbox(30, 110)), vec![11]);
        // A virtual list: tall, with only a few rows rendered.
        let mut virtual_list = inbox(3, 110);
        if let Some(bounds) = virtual_list
            .iter_mut()
            .find(|node| node.index == 11)
            .and_then(|list| list.bounds.as_mut())
        {
            bounds.height = 80_000;
        }
        assert_eq!(scrollable_lists(&virtual_list), vec![11]);
        // Five rows fit; an empty list has nothing to scroll.
        assert!(scrollable_lists(&inbox(5, 110)).is_empty());
        let empty = browser(vec![at(
            node(11, Some(10), "list", "Vazia"),
            0,
            110,
            1000,
            2000,
        )]);
        assert!(scrollable_lists(&empty).is_empty());
    }

    #[test]
    fn rows_below_the_fold_are_drawn_with_a_marker_within_the_budget() {
        let view = page_view(&inbox(30, 110), ViewOptions::default());
        let outline = view.outline();
        assert!(
            outline.contains("    [117] list item \"Mensagem 017\" @0,790 1000x40\n"),
            "{outline}"
        );
        assert!(
            outline.contains("    [118] list item \"Mensagem 018\" (below)\n"),
            "{outline}"
        );
        assert!(
            outline.contains("    [129] list item \"Mensagem 029\" (below)\n"),
            "{outline}"
        );
        assert!(!outline.contains("omitted"), "{outline}");
        assert_eq!(view.entries.len(), 32);
        // Off-screen rows do not count as content a screenshot could replace.
        assert_eq!(view.content_elements(), 1 + 1 + 18);

        // Scrolled down: the first rows are above, beside rows are off-screen.
        let mut scrolled = inbox(30, -1090);
        scrolled.push(at(
            node(90, Some(10), "button", "Painel"),
            1200,
            300,
            40,
            40,
        ));
        let view = page_view(&scrolled, ViewOptions::default());
        let line = |id: u32| {
            view.entries
                .iter()
                .find(|entry| entry.id == id)
                .map(|entry| entry.line.clone())
        };
        assert_eq!(
            line(100).as_deref(),
            Some("[100] list item \"Mensagem 000\" (above)")
        );
        assert_eq!(
            line(129).as_deref(),
            Some("[129] list item \"Mensagem 029\" @0,70 1000x40")
        );
        assert_eq!(
            line(90).as_deref(),
            Some("[90] button \"Painel\" (off-screen)")
        );
    }

    #[test]
    fn over_the_budget_the_nearest_outside_rows_are_drawn_and_the_rest_counted() {
        let view = page_view(&inbox(400, 110), ViewOptions::default());
        assert_eq!(view.entries.len(), OUTLINE_MAX_LINES);
        assert_eq!(view.omitted.len(), 402 - OUTLINE_MAX_LINES);
        // Nearest first: the drawn rows are the first ones below the fold.
        let last = view.entries.last().map(|entry| entry.id);
        assert_eq!(last, Some(100 + OUTLINE_MAX_LINES as u32 - 3));
        assert!(view.omitted.contains(&(100 + OUTLINE_MAX_LINES as u32 - 2)));
        let outline = view.outline();
        assert!(outline.ends_with(&format!(
            "({} elements outside the visible window omitted; find_elements searches them)\n",
            402 - OUTLINE_MAX_LINES
        )));
        assert!(
            outline.len() <= OUTLINE_MAX_BYTES + 200,
            "{}",
            outline.len()
        );
    }

    #[test]
    fn budget_takes_every_visible_line_then_the_nearest_outside_ones() {
        // (distance outside, bytes): visible lines always fit, even over budget.
        let costs = [
            (None, 50),
            (Some(300), 10),
            (Some(0), 10),
            (None, 50),
            (Some(10), 10),
            (Some(10), 10),
        ];
        assert_eq!(
            within_budget(&costs, 4, 1000),
            [true, false, true, true, true, false]
        );
        assert_eq!(
            within_budget(&costs, 10, 130),
            [true, false, true, true, true, true]
        );
        // The first outside line that does not fit ends the selection, so a
        // nearer long line is not skipped for farther short ones.
        let costs = [(None, 10), (Some(1), 500), (Some(2), 5)];
        assert_eq!(within_budget(&costs, 10, 100), [true, false, false]);
        assert_eq!(within_budget(&[(None, 900)], 0, 10), [true]);
    }

    #[test]
    fn change_reports_count_appended_rows_and_rows_scrolled_into_view() {
        let before = page_view(&inbox(20, 110), ViewOptions::default());
        // Twenty rows appended below the fold are new.
        let after = page_view(&inbox(40, 110), ViewOptions::default());
        let report = change_report(&before, &after, &[]);
        assert!(report.starts_with("changes (20):\n"), "{report}");
        assert!(
            report.contains("+ [139] list item \"Mensagem 039\" (below)\n"),
            "{report}"
        );
        // Scrolled so rows 18-19 come into view: they changed; rows 0-2
        // left the view but are still there and say nothing.
        let mut scrolled = inbox(20, -10);
        if let Some(list) = scrolled.iter_mut().find(|node| node.index == 11) {
            list.bounds = Some(Bounds {
                x: 0,
                y: 110,
                width: 1000,
                height: 690,
            });
        }
        let report = change_report(&before, &page_view(&scrolled, ViewOptions::default()), &[]);
        assert_eq!(
            report,
            "changes (2):\n\
             ~ [118] list item \"Mensagem 018\" @0,710 1000x40\n\
             ~ [119] list item \"Mensagem 019\" @0,750 1000x40\n"
        );
        // A row left out for the budget is not gone.
        let big = page_view(&inbox(400, 110), ViewOptions::default());
        assert!(change_report(&big, &big, &[]).starts_with("changes: none"));
        let mut fewer = big.clone();
        fewer.entries.retain(|entry| entry.id != 300);
        fewer.omitted.insert(300);
        let mut drawn_before = big;
        drawn_before.omitted.remove(&300);
        assert_eq!(change_report(&drawn_before, &fewer, &[]), "changes: none\n");
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
