use crate::diagnostics::hydrate_session_bus_env;
use anyhow::{anyhow, Context, Result};
use atspi::{
    proxy::{
        accessible::{AccessibleProxy, ObjectRefExt},
        proxy_ext::ProxyExt,
    },
    CoordType, ObjectRef, ObjectRefOwned, StateSet,
};
// Direct dependency (p2p feature off) — see Cargo.toml for why we bypass
// atspi's "connection" re-export.
use atspi_connection::AccessibilityConnection;
use futures_util::{stream, StreamExt};
use schemars::JsonSchema;
use serde::Serialize;
use std::{collections::VecDeque, future::Future, time::Duration};
use tokio::time::timeout;
use zbus::{
    fdo::DBusProxy,
    names::{BusName, UniqueName},
    zvariant::ObjectPath,
};

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AccessibleAppSummary {
    pub object_ref: String,
    pub name: Option<String>,
    pub pid: Option<u32>,
    pub role: String,
    pub child_count: i32,
    pub bounds: Option<Bounds>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AccessibilityNode {
    pub index: u32,
    pub parent_index: Option<u32>,
    pub depth: u32,
    pub object_ref: String,
    pub role: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub child_count: i32,
    pub bounds: Option<Bounds>,
    pub states: Vec<String>,
    pub actions: Vec<AccessibilityAction>,
    pub value: Option<AccessibilityValue>,
    pub text: Option<AccessibilityText>,
    pub supports_editable_text: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AccessibilityAction {
    pub index: i32,
    pub name: String,
    pub description: String,
    pub keybinding: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AccessibilityValue {
    pub current: f64,
    pub minimum: f64,
    pub maximum: f64,
    pub minimum_increment: f64,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AccessibilityText {
    pub character_count: i32,
    pub caret_offset: Option<i32>,
    pub content: Option<String>,
    pub truncated: bool,
    pub selections: Vec<AccessibilityTextSelection>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AccessibilityTextSelection {
    pub start_offset: i32,
    pub end_offset: i32,
}

#[derive(Debug, Clone)]
pub struct ActionInvocation {
    pub action_index: i32,
    pub action_name: Option<String>,
    pub ok: bool,
}

#[derive(Debug, Clone)]
pub enum ValueSetInvocation {
    Numeric { value: f64 },
    EditableText,
}

const MAX_TEXT_READBACK_CHARS: i32 = 4096;
const MAX_TEXT_SELECTIONS: i32 = 8;
const DEFAULT_SNAPSHOT_MAX_NODES: usize = 1_000;
const HARD_SNAPSHOT_MAX_NODES: usize = 2_000;
const DEFAULT_SNAPSHOT_MAX_DEPTH: u32 = 32;
const HARD_SNAPSHOT_MAX_DEPTH: u32 = 64;
const CHILD_READ_CONCURRENCY: usize = 16;
/// Nodes whose properties a snapshot reads at the same time.
const NODE_READ_BATCH: usize = 32;
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DISCOVERY_ROOTS: usize = 256;
const ROOT_MATCH_CHILD_LIMIT: usize = 8;
const MAX_DISCOVERY_CHILD_READS: usize = MAX_DISCOVERY_ROOTS * ROOT_MATCH_CHILD_LIMIT;

fn snapshot_child_read_budgets(max_nodes: usize) -> (usize, usize, usize) {
    (MAX_DISCOVERY_ROOTS, MAX_DISCOVERY_CHILD_READS, max_nodes)
}

pub(crate) fn snapshot_limits(
    requested_max_nodes: Option<usize>,
    requested_max_depth: Option<u32>,
) -> (usize, u32) {
    (
        requested_max_nodes
            .unwrap_or(DEFAULT_SNAPSHOT_MAX_NODES)
            .clamp(1, HARD_SNAPSHOT_MAX_NODES),
        requested_max_depth
            .unwrap_or(DEFAULT_SNAPSHOT_MAX_DEPTH)
            .min(HARD_SNAPSHOT_MAX_DEPTH),
    )
}

struct BoundedTraversal<T> {
    queue: VecDeque<T>,
    attempted: usize,
    max_items: usize,
    /// Set once an enqueue had to drop offered items for lack of capacity.
    dropped: bool,
}

impl<T> BoundedTraversal<T> {
    fn new(max_items: usize) -> Self {
        Self {
            queue: VecDeque::new(),
            attempted: 0,
            max_items,
            dropped: false,
        }
    }

    fn enqueue(&mut self, items: impl IntoIterator<Item = T>) {
        let capacity = self.remaining_capacity();
        let mut items = items.into_iter();
        self.queue.extend(items.by_ref().take(capacity));
        if items.next().is_some() {
            self.dropped = true;
        }
    }

    fn pop(&mut self) -> Option<T> {
        if self.attempted >= self.max_items {
            return None;
        }
        let item = self.queue.pop_front()?;
        self.attempted += 1;
        Some(item)
    }

    fn remaining_capacity(&self) -> usize {
        self.max_items
            .saturating_sub(self.attempted.saturating_add(self.queue.len()))
    }

    /// True when the budget stopped work that was actually offered. `enqueue`
    /// is the only gate: it never exceeds `remaining_capacity`, so
    /// `attempted + queue.len() <= max_items` always holds and the queue can
    /// never be non-empty at the attempt cap. Dropped items are the whole story.
    fn truncated(&self) -> bool {
        self.dropped
    }
}

fn bounded_child_count(reported: i32, limit: usize) -> usize {
    usize::try_from(reported).unwrap_or_default().min(limit)
}

struct IndexedReadBatch<T> {
    items: Vec<T>,
    attempted: usize,
    /// True when fewer children were attempted than the parent reported, because
    /// the caller's limit or the shared read budget ran out. Failed reads do not
    /// count; they were attempted.
    incomplete: bool,
}

impl<T> IndexedReadBatch<T> {
    fn all_failed(&self) -> bool {
        self.attempted > 0 && self.items.is_empty()
    }
}

async fn fetch_indexed_up_to<T, E, F, Fut>(
    reported: i32,
    limit: usize,
    remaining_attempts: &mut usize,
    fetch: F,
) -> IndexedReadBatch<T>
where
    F: Fn(i32) -> Fut,
    Fut: Future<Output = std::result::Result<T, E>>,
{
    let attempt_count = bounded_child_count(reported, limit).min(*remaining_attempts);
    *remaining_attempts = (*remaining_attempts).saturating_sub(attempt_count);
    let end_index = i32::try_from(attempt_count).unwrap_or(i32::MAX);

    let items = stream::iter(0..end_index)
        .map(fetch)
        .buffered(CHILD_READ_CONCURRENCY)
        .filter_map(|result| async move { result.ok() })
        .collect()
        .await;

    IndexedReadBatch {
        items,
        attempted: attempt_count,
        incomplete: attempt_count < usize::try_from(reported).unwrap_or_default(),
    }
}

async fn children_up_to(
    proxy: &AccessibleProxy<'_>,
    limit: usize,
    remaining_attempts: &mut usize,
) -> zbus::Result<IndexedReadBatch<ObjectRefOwned>> {
    if limit == 0 || *remaining_attempts == 0 {
        // Budget already spent: stay I/O-free. `incomplete` is false here because
        // nothing was asked of the parent; callers that already hold the node's
        // child_count (snapshot_tree_inner) derive truncation from that instead.
        return Ok(IndexedReadBatch {
            items: Vec::new(),
            attempted: 0,
            incomplete: false,
        });
    }

    let child_count = proxy.child_count().await?;
    Ok(
        fetch_indexed_up_to(child_count, limit, remaining_attempts, |index| {
            proxy.get_child_at_index(index)
        })
        .await,
    )
}

pub async fn list_accessible_apps(limit: usize) -> Result<Vec<AccessibleAppSummary>> {
    let conn = connect().await?;
    let mut remaining_child_reads = limit;
    let roots = registry_children(&conn, limit, &mut remaining_child_reads).await?;
    let dbus = DBusProxy::new(conn.connection()).await.ok();
    let mut apps = Vec::new();

    for object_ref in roots.into_iter().take(limit) {
        if let Ok(proxy) = open_accessible(&conn, &object_ref).await {
            apps.push(read_app_summary(&proxy, &object_ref, dbus.as_ref()).await);
        }
    }

    Ok(apps)
}

#[derive(Debug, Clone)]
pub(crate) struct AccessibilitySnapshot {
    pub nodes: Vec<AccessibilityNode>,
    /// True when registry roots were filtered to an app name and/or pid.
    pub scoped: bool,
    /// The pid every root belongs to, when roots were matched by pid. None
    /// when they were matched by app name alone or not filtered at all: those
    /// roots can belong to any app, so no single pid describes the tree.
    pub root_pid: Option<u32>,
    /// True when max_nodes, max_depth, or the child read budget stopped
    /// traversal with unread elements left. Failed element reads do not count.
    pub truncated: bool,
}

pub async fn snapshot_tree(
    app_name_or_bundle_identifier: Option<&str>,
    target_pid: Option<u32>,
    max_nodes: usize,
    max_depth: u32,
) -> Result<Vec<AccessibilityNode>> {
    Ok(snapshot_accessibility_tree(
        app_name_or_bundle_identifier,
        target_pid,
        max_nodes,
        max_depth,
    )
    .await?
    .nodes)
}

pub(crate) async fn snapshot_accessibility_tree(
    app_name_or_bundle_identifier: Option<&str>,
    target_pid: Option<u32>,
    max_nodes: usize,
    max_depth: u32,
) -> Result<AccessibilitySnapshot> {
    let (max_nodes, max_depth) = snapshot_limits(Some(max_nodes), Some(max_depth));
    timeout(
        SNAPSHOT_TIMEOUT,
        snapshot_tree_inner(
            app_name_or_bundle_identifier,
            target_pid,
            max_nodes,
            max_depth,
        ),
    )
    .await
    .context("AT-SPI snapshot exceeded its 10-second deadline")?
}

async fn snapshot_tree_inner(
    app_name_or_bundle_identifier: Option<&str>,
    target_pid: Option<u32>,
    max_nodes: usize,
    max_depth: u32,
) -> Result<AccessibilitySnapshot> {
    let conn = connect().await?;
    // App discovery is bounded independently so a tiny requested tree still
    // finds a target registered after the first accessibility root.
    let (mut remaining_registry_reads, mut remaining_filter_reads, remaining_traversal_reads) =
        snapshot_child_read_budgets(max_nodes);
    let roots =
        registry_children(&conn, MAX_DISCOVERY_ROOTS, &mut remaining_registry_reads).await?;
    let selected_roots = select_roots(
        &conn,
        roots,
        app_name_or_bundle_identifier,
        target_pid,
        &mut remaining_filter_reads,
    )
    .await;
    let scoped = selected_roots.scoped;
    let root_pid = selected_roots.pid;
    let roots = selected_roots
        .roots
        .into_iter()
        .map(|object_ref| (object_ref, 0_u32))
        .collect();
    let (nodes, truncated) = traverse(
        &conn,
        roots,
        max_nodes,
        max_depth,
        remaining_traversal_reads,
    )
    .await;

    Ok(AccessibilitySnapshot {
        nodes,
        scoped,
        root_pid,
        truncated,
    })
}

/// Fresh read of an element and its descendants as a snapshot reads them:
/// breadth-first in batches, `depth` being the element's depth in its app's
/// tree, so `max_depth` and compaction treat the nodes as they would in a
/// full snapshot. `index`/`parent_index` are positions in the result, the
/// element first. Fails after `SNAPSHOT_TIMEOUT` or when the element cannot
/// be read.
pub(crate) async fn snapshot_subtree(
    object_ref_id: &str,
    depth: u32,
    max_nodes: usize,
    max_depth: u32,
) -> Result<Vec<AccessibilityNode>> {
    let (max_nodes, max_depth) = snapshot_limits(Some(max_nodes), Some(max_depth));
    let read = async {
        let conn = connect().await?;
        let root = object_ref_from_id(object_ref_id)?;
        let proxy = open_accessible(&conn, &root).await?;
        // A vanished object answers every read with an error, which read_node
        // would turn into an empty "unknown" node.
        proxy
            .get_state()
            .await
            .with_context(|| format!("AT-SPI object {object_ref_id} no longer exists"))?;
        drop(proxy);
        let (nodes, _) =
            traverse(&conn, vec![(root, depth)], max_nodes, max_depth, max_nodes).await;
        anyhow::Ok(nodes)
    };
    timeout(SNAPSHOT_TIMEOUT, read)
        .await
        .context("AT-SPI subtree read exceeded its 10-second deadline")?
}

/// Reads `roots` (object, depth) and their descendants breadth-first, at
/// most `max_nodes` nodes, `max_depth` levels and `child_reads` child
/// listings. Returns the nodes, indexed by position, and whether a limit
/// left elements unread.
async fn traverse(
    conn: &AccessibilityConnection,
    roots: Vec<(ObjectRefOwned, u32)>,
    max_nodes: usize,
    max_depth: u32,
    child_reads: usize,
) -> (Vec<AccessibilityNode>, bool) {
    let mut remaining_traversal_reads = child_reads;
    let mut nodes = Vec::new();
    let mut truncated = false;
    let mut traversal = BoundedTraversal::new(max_nodes);
    traversal.enqueue(
        roots
            .into_iter()
            .map(|(object_ref, depth)| (object_ref, depth, None)),
    );

    // Breadth-first in batches: every node of a batch is read concurrently,
    // then their children are listed concurrently. Results keep queue order,
    // so indices and parent links match a one-at-a-time traversal.
    loop {
        let batch: Vec<_> = std::iter::from_fn(|| traversal.pop())
            .take(NODE_READ_BATCH)
            .collect();
        if batch.is_empty() {
            break;
        }
        let read: Vec<_> = stream::iter(batch)
            .map(|(object_ref, depth, parent_index)| async move {
                let proxy = open_accessible(conn, &object_ref).await.ok()?;
                let node = read_node(&proxy, &object_ref, 0, parent_index, depth).await;
                drop(proxy);
                Some((object_ref, depth, node))
            })
            .buffered(NODE_READ_BATCH)
            .collect()
            .await;

        // Split the node cap and the child read budget in queue order, as the
        // sequential traversal did; `None` marks a node whose children stay
        // unread because of the depth or node cap.
        let mut remaining = traversal.remaining_capacity();
        let planned: Vec<_> = read
            .into_iter()
            .flatten()
            .map(|(object_ref, depth, node)| {
                let attempts = (depth < max_depth && remaining > 0).then(|| {
                    let attempts = bounded_child_count(node.child_count, remaining)
                        .min(remaining_traversal_reads);
                    remaining -= attempts;
                    remaining_traversal_reads -= attempts;
                    attempts
                });
                (object_ref, depth, node, attempts)
            })
            .collect();
        let listings: Vec<_> = planned
            .iter()
            .map(|(object_ref, _, node, attempts)| {
                (object_ref.clone(), node.child_count, *attempts)
            })
            .collect();
        let children: Vec<_> = stream::iter(listings)
            .map(|(object_ref, child_count, attempts)| async move {
                let attempts = attempts?;
                let proxy = open_accessible(conn, &object_ref).await.ok()?;
                let mut budget = attempts;
                let listed = fetch_indexed_up_to(child_count, attempts, &mut budget, |index| {
                    proxy.get_child_at_index(index)
                })
                .await;
                drop(proxy);
                Some(listed)
            })
            .buffered(NODE_READ_BATCH)
            .collect()
            .await;

        for ((_, depth, mut node, attempts), listed) in planned.into_iter().zip(children) {
            let index = nodes.len() as u32;
            node.index = index;
            let child_refs = match (attempts, listed) {
                // A cut by the node cap or read budget reports `incomplete`.
                (Some(_), Some(listed)) => {
                    truncated |= listed.incomplete;
                    listed.items
                }
                (Some(_), None) => Vec::new(),
                // Depth or node cap reached: any child this node reports is unread.
                (None, _) => {
                    truncated |= node.child_count > 0;
                    Vec::new()
                }
            };
            nodes.push(node);
            traversal.enqueue(
                child_refs
                    .into_iter()
                    .map(|child| (child, depth + 1, Some(index))),
            );
        }
    }
    truncated |= traversal.truncated();
    (nodes, truncated)
}

/// Compact description of the AT-SPI element that currently holds keyboard
/// focus, used as post-input feedback for type_text/press_key.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FocusedElementSummary {
    pub role: String,
    pub name: Option<String>,
    pub editable: bool,
    pub states: Vec<String>,
    /// AT-SPI `Role::Terminal`, read from the role enum rather than the
    /// localized role name. Internal routing only; not part of the output.
    #[serde(skip)]
    #[schemars(skip)]
    pub is_terminal: bool,
    /// `:bus/path` of the element, for reading its text back. Internal only.
    #[serde(skip)]
    #[schemars(skip)]
    pub object_ref: String,
}

const FOCUS_PROBE_MAX_NODES: usize = 400;
const FOCUS_PROBE_MAX_DEPTH: u32 = 16;

/// Find the element with the `focused` state inside the target app (by pid) or
/// across all apps. Best-effort and bounded: returns Ok(None) when no focused
/// element is reachable through AT-SPI (common for apps without accessibility
/// support, e.g. Electron without --force-renderer-accessibility).
/// Outcome of the bounded focused-element search.
#[derive(Debug, Clone)]
pub(crate) enum FocusProbe {
    Found(FocusedElementSummary),
    /// The search covered the target's whole tree and nothing holds focus.
    NoneFocused,
    /// A complete scoped search saw only window/menu chrome, no content.
    ShellOnly,
    /// A node, depth, or read limit stopped the search before it finished, so
    /// the focused element may exist past the limit.
    Incomplete,
    /// A pid was given but no AT-SPI app belongs to it (xterm, or Electron
    /// without --force-renderer-accessibility), so focus cannot be read.
    NoAccessibleApp,
}

/// Focused element in the app owning `target_pid`, or across all apps when no
/// pid is given. With a pid, only that app's tree is searched: falling back to
/// every other app would report some unrelated widget as the focus.
pub(crate) async fn probe_focused_element(target_pid: Option<u32>) -> Result<FocusProbe> {
    focused_element_probe(target_pid, target_pid.is_some()).await
}

async fn focused_element_probe(
    target_pid: Option<u32>,
    require_scoped: bool,
) -> Result<FocusProbe> {
    let conn = connect().await?;
    let mut remaining_registry_reads = MAX_DISCOVERY_ROOTS;
    let roots =
        registry_children(&conn, MAX_DISCOVERY_ROOTS, &mut remaining_registry_reads).await?;
    let mut remaining_filter_reads = MAX_DISCOVERY_CHILD_READS;
    let selected_roots =
        select_roots(&conn, roots, None, target_pid, &mut remaining_filter_reads).await;
    if require_scoped && !selected_roots.scoped {
        return Ok(FocusProbe::NoAccessibleApp);
    }
    let mut traversal = BoundedTraversal::new(FOCUS_PROBE_MAX_NODES);
    let mut remaining_traversal_reads = FOCUS_PROBE_MAX_NODES;
    let mut incomplete = false;
    let mut shell_only = true;
    let mut observed_nodes = 0;

    traversal.enqueue(
        selected_roots
            .roots
            .into_iter()
            .map(|object_ref| (object_ref, 0_u32, false)),
    );

    while let Some((object_ref, depth, in_menu)) = traversal.pop() {
        let Ok(proxy) = open_accessible(&conn, &object_ref).await else {
            incomplete = true;
            continue;
        };
        let Ok(state) = proxy.get_state().await else {
            incomplete = true;
            continue;
        };
        observed_nodes += 1;
        let mut child_menu = false;
        if shell_only {
            match accessibility_chrome_context(&role_name(&proxy).await, in_menu) {
                Some(menu) => child_menu = menu,
                None => shell_only = false,
            }
        }
        if state.contains(atspi::State::Focused) {
            let proxies = proxy.proxies().await.ok();
            let is_terminal = matches!(proxy.get_role().await, Ok(atspi::Role::Terminal));
            return Ok(FocusProbe::Found(FocusedElementSummary {
                role: role_name(&proxy).await,
                name: optional_string(proxy.name().await.ok()),
                editable: supports_editable_text(proxies.as_ref()).await,
                states: state_labels(state),
                is_terminal,
                object_ref: object_ref_id(&object_ref),
            }));
        }
        let remaining = traversal.remaining_capacity();
        if depth < FOCUS_PROBE_MAX_DEPTH && remaining > 0 {
            if let Ok(batch) =
                children_up_to(&proxy, remaining, &mut remaining_traversal_reads).await
            {
                incomplete |= batch.incomplete;
                // An exhausted shared read budget takes children_up_to's
                // I/O-free path (attempted 0, incomplete false), so the node's
                // own child count has to say whether children went unread.
                // With budget left, attempted 0 just means a leaf: no read.
                if batch.attempted == 0 && remaining_traversal_reads == 0 && !incomplete {
                    incomplete = proxy.child_count().await.map_or(true, |count| count > 0);
                }
                traversal.enqueue(
                    batch
                        .items
                        .into_iter()
                        .map(|child| (child, depth + 1, child_menu)),
                );
            } else {
                incomplete = true;
            }
        } else if !incomplete {
            // Depth or node cap: any child this node reports goes unread. One
            // child-count read settles it, and only until the first cut is seen.
            incomplete = proxy.child_count().await.map_or(true, |count| count > 0);
        }
    }
    if incomplete || traversal.truncated() {
        return Ok(FocusProbe::Incomplete);
    }

    Ok(if require_scoped && shell_only && observed_nodes > 0 {
        FocusProbe::ShellOnly
    } else {
        FocusProbe::NoneFocused
    })
}

/// Window and native menu chrome can exist without Electron renderer content.
/// This is evidence about tree coverage, not positive Electron detection.
pub(crate) fn accessibility_chrome_context(role: &str, in_menu: bool) -> Option<bool> {
    match role {
        "application" | "frame" | "window" | "panel" | "filler" | "separator" => Some(in_menu),
        "menu bar" | "menu" | "menu item" | "check menu item" | "radio menu item"
        | "tearoff menu item" => Some(true),
        // Electron's native menu buttons are exposed as buttons, not menu items.
        "button" | "push button" | "label" | "static" | "image" | "icon" if in_menu => Some(true),
        _ => None,
    }
}

pub async fn perform_action(
    object_ref_id: &str,
    requested_action: Option<&str>,
) -> Result<ActionInvocation> {
    perform_action_inner(object_ref_id, requested_action, false).await
}

pub(crate) async fn perform_named_action(
    object_ref_id: &str,
    name: &str,
) -> Result<ActionInvocation> {
    perform_action_inner(object_ref_id, Some(name), true).await
}

async fn perform_action_inner(
    object_ref_id: &str,
    requested_action: Option<&str>,
    name_only: bool,
) -> Result<ActionInvocation> {
    let conn = connect().await?;
    let object_ref = object_ref_from_id(object_ref_id)?;
    let proxy = open_accessible(&conn, &object_ref)
        .await
        .with_context(|| format!("failed to open AT-SPI object {object_ref_id}"))?;
    let action = proxy
        .proxies()
        .await?
        .action()
        .await
        .context("element does not expose the AT-SPI Action interface")?;
    let mut actions = action.get_actions().await.unwrap_or_default();
    for (index, entry) in actions.iter_mut().enumerate() {
        if entry.name.is_empty() {
            if let Ok(name) = action.get_name(index as i32).await {
                entry.name = name;
            }
        }
    }
    let action_index = if name_only {
        select_named_action_index(&actions, requested_action.unwrap_or_default())?
    } else {
        select_action_index(&actions, requested_action)?
    };
    let action_name = actions
        .get(action_index as usize)
        .map(|action| action.name.clone());
    let ok = action
        .do_action(action_index)
        .await
        .with_context(|| format!("failed to invoke AT-SPI action {action_index}"))?;

    Ok(ActionInvocation {
        action_index,
        action_name,
        ok,
    })
}

/// Fresh read of one element by object ref. `index`, `parent_index` and
/// `depth` are zero: the node is not part of any snapshot.
pub(crate) async fn read_element(object_ref_id: &str) -> Result<AccessibilityNode> {
    let conn = connect().await?;
    let object_ref = object_ref_from_id(object_ref_id)?;
    let proxy = open_accessible(&conn, &object_ref)
        .await
        .with_context(|| format!("failed to open AT-SPI object {object_ref_id}"))?;
    // A vanished object answers every property read with an error, which
    // read_node would turn into an empty "unknown" node; check one first.
    proxy
        .get_state()
        .await
        .with_context(|| format!("AT-SPI object {object_ref_id} no longer exists"))?;
    Ok(read_node(&proxy, &object_ref, 0, None, 0).await)
}

/// Object ref of an element's parent.
pub(crate) async fn parent_ref(id: &str) -> Result<String> {
    let conn = connect().await?;
    let object_ref = object_ref_from_id(id)?;
    let proxy = open_accessible(&conn, &object_ref)
        .await
        .with_context(|| format!("failed to open AT-SPI object {id}"))?;
    let parent = proxy
        .parent()
        .await
        .with_context(|| format!("AT-SPI object {id} no longer exists"))?;
    Ok(object_ref_id(&parent))
}

/// Fresh read of an element and its descendants, breadth-first, at most
/// `max_nodes` nodes and `max_depth` levels below it. `index` is each node's
/// position in the result and `parent_index` its parent's, so `nodes[0]` is
/// the element and children follow their parents in sibling order.
pub(crate) async fn read_subtree(
    object_ref_id: &str,
    max_nodes: usize,
    max_depth: u32,
) -> Result<Vec<AccessibilityNode>> {
    let conn = connect().await?;
    let root = object_ref_from_id(object_ref_id)?;
    let mut queue = VecDeque::from([(root, 0_u32, None)]);
    let mut nodes = Vec::new();
    let mut remaining_reads = max_nodes;
    while let Some((object_ref, depth, parent_index)) = queue.pop_front() {
        if nodes.len() >= max_nodes {
            break;
        }
        let Ok(proxy) = open_accessible(&conn, &object_ref).await else {
            continue;
        };
        let index = u32::try_from(nodes.len()).unwrap_or(u32::MAX);
        let node = read_node(&proxy, &object_ref, index, parent_index, depth).await;
        if depth < max_depth {
            let budget = max_nodes.saturating_sub(nodes.len() + 1 + queue.len());
            if let Ok(children) = children_up_to(&proxy, budget, &mut remaining_reads).await {
                queue.extend(
                    children
                        .items
                        .into_iter()
                        .map(|child| (child, depth + 1, Some(index))),
                );
            }
        }
        nodes.push(node);
    }
    if nodes.is_empty() {
        return Err(anyhow!("AT-SPI object {object_ref_id} no longer exists"));
    }
    Ok(nodes)
}

/// Moves keyboard focus to the element through `Component.GrabFocus`.
/// Returns the toolkit's answer; callers still read the `focused` state back.
pub(crate) async fn grab_focus(object_ref_id: &str) -> Result<bool> {
    let conn = connect().await?;
    let object_ref = object_ref_from_id(object_ref_id)?;
    let proxy = open_accessible(&conn, &object_ref).await?;
    let component = proxy
        .proxies()
        .await?
        .component()
        .await
        .context("element does not expose the AT-SPI Component interface")?;
    Ok(component.grab_focus().await?)
}

/// Asks the toolkit to scroll the element into view (`Component.ScrollTo`).
pub(crate) async fn scroll_into_view(object_ref_id: &str) -> Result<bool> {
    let conn = connect().await?;
    let object_ref = object_ref_from_id(object_ref_id)?;
    let proxy = open_accessible(&conn, &object_ref).await?;
    let component = proxy
        .proxies()
        .await?
        .component()
        .await
        .context("element does not expose the AT-SPI Component interface")?;
    Ok(component.scroll_to(atspi::ScrollType::Anywhere).await?)
}

/// Invokes the first action whose name appears in `preferred` (in that
/// order, case-insensitive). `Ok(None)` when the element has none of them.
pub(crate) async fn invoke_preferred_action(
    object_ref_id: &str,
    preferred: &[&str],
) -> Result<Option<ActionInvocation>> {
    let conn = connect().await?;
    let object_ref = object_ref_from_id(object_ref_id)?;
    let proxy = open_accessible(&conn, &object_ref)
        .await
        .with_context(|| format!("failed to open AT-SPI object {object_ref_id}"))?;
    let Ok(action) = proxy.proxies().await?.action().await else {
        return Ok(None);
    };
    let mut actions = action.get_actions().await.unwrap_or_default();
    for (index, entry) in actions.iter_mut().enumerate() {
        if entry.name.is_empty() {
            if let Ok(name) = action.get_name(index as i32).await {
                entry.name = name;
            }
        }
    }
    let Some(action_index) = preferred.iter().find_map(|wanted| {
        actions
            .iter()
            .position(|action| action.name.eq_ignore_ascii_case(wanted))
    }) else {
        return Ok(None);
    };
    let ok = action
        .do_action(action_index as i32)
        .await
        .with_context(|| format!("failed to invoke AT-SPI action {action_index}"))?;
    Ok(Some(ActionInvocation {
        action_index: action_index as i32,
        action_name: actions.get(action_index).map(|action| action.name.clone()),
        ok,
    }))
}

pub async fn set_element_value(object_ref_id: &str, value: &str) -> Result<ValueSetInvocation> {
    let conn = connect().await?;
    let object_ref = object_ref_from_id(object_ref_id)?;
    let proxy = open_accessible(&conn, &object_ref)
        .await
        .with_context(|| format!("failed to open AT-SPI object {object_ref_id}"))?;
    let proxies = proxy.proxies().await?;

    if let Ok(numeric_value) = value.parse::<f64>() {
        if let Ok(value_proxy) = proxies.value().await {
            value_proxy
                .set_current_value(numeric_value)
                .await
                .with_context(|| {
                    format!("failed to set AT-SPI numeric value to {numeric_value}")
                })?;
            return Ok(ValueSetInvocation::Numeric {
                value: numeric_value,
            });
        }
    }

    if let Ok(editable_text) = proxies.editable_text().await {
        let ok = editable_text
            .set_text_contents(value)
            .await
            .context("failed to set AT-SPI editable text contents")?;
        if ok {
            return Ok(ValueSetInvocation::EditableText);
        }
        return Err(anyhow!("AT-SPI EditableText rejected the new contents"));
    }

    if value.parse::<f64>().is_err() && proxies.value().await.is_ok() {
        return Err(anyhow!(
            "element exposes the AT-SPI Value interface, but the requested value is not numeric"
        ));
    }

    Err(anyhow!(
        "element does not expose AT-SPI Value or EditableText interfaces"
    ))
}

/// The a11y bus connection, opened once and shared by every read. Opening one
/// costs a bus handshake per call otherwise.
static CONNECTION: std::sync::Mutex<Option<AccessibilityConnection>> = std::sync::Mutex::new(None);

async fn connect() -> Result<AccessibilityConnection> {
    if let Some(conn) = CONNECTION.lock().ok().and_then(|cached| cached.clone()) {
        return Ok(conn);
    }
    hydrate_session_bus_env();
    let conn = AccessibilityConnection::new()
        .await
        .context("failed to connect to AT-SPI bus")?;
    if let Ok(mut cached) = CONNECTION.lock() {
        *cached = Some(conn.clone());
    }
    Ok(conn)
}

/// Drops the shared connection so the next read reconnects; called when the
/// registry stops answering (bus restarted, or the runtime that owned the
/// connection's reader task is gone).
fn forget_connection() {
    if let Ok(mut cached) = CONNECTION.lock() {
        *cached = None;
    }
}

/// Open an `AccessibleProxy` for an object on the a11y bus.
///
/// We deliberately avoid `AccessibilityConnection::object_as_accessible` (the
/// `P2P` trait). For apps that advertise a peer-to-peer bus address it routes
/// reads over that socket, but for apps that don't (notably GTK4 apps such as
/// Nautilus / Text Editor / baobab, which don't implement the legacy
/// `GetApplicationBusAddress`) it falls back to a proxy built with only a path
/// and *no destination*. On the shared a11y bus that proxy can't address the
/// app and every call fails with `ServiceUnknown`, which surfaces as an empty
/// tree (`role: "unknown"`, `child_count: 0`). `as_accessible_proxy` always
/// pins the destination to the object's bus name, so it works for every app
/// regardless of P2P support. See issue #31.
async fn open_accessible<'r>(
    conn: &AccessibilityConnection,
    object_ref: &'r ObjectRefOwned,
) -> Result<AccessibleProxy<'r>, atspi::AtspiError> {
    object_ref.as_accessible_proxy(conn.connection()).await
}

async fn registry_children(
    conn: &AccessibilityConnection,
    limit: usize,
    remaining_child_reads: &mut usize,
) -> Result<Vec<ObjectRefOwned>> {
    let root = conn
        .root_accessible_on_registry()
        .await
        .context("failed to open AT-SPI registry root")?;
    let batch = match children_up_to(&root, limit, remaining_child_reads).await {
        Ok(batch) => batch,
        Err(error) => {
            forget_connection();
            return Err(anyhow!(error).context("failed to read AT-SPI registry children"));
        }
    };
    if batch.all_failed() {
        return Err(anyhow!(
            "AT-SPI registry reported children, but every indexed child read failed"
        ));
    }
    Ok(batch.items)
}

struct SelectedRoots {
    roots: Vec<ObjectRefOwned>,
    scoped: bool,
    /// Set only when the roots were chosen because they belong to this pid.
    pid: Option<u32>,
}

/// Normalized app-name filter, or `None` when the caller passed nothing usable.
/// A `None` needle with no pid match means the snapshot covers the whole desktop.
fn app_name_needle(app_name_or_bundle_identifier: Option<&str>) -> Option<String> {
    app_name_or_bundle_identifier
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

async fn select_roots(
    conn: &AccessibilityConnection,
    roots: Vec<ObjectRefOwned>,
    app_name_or_bundle_identifier: Option<&str>,
    target_pid: Option<u32>,
    remaining_child_reads: &mut usize,
) -> SelectedRoots {
    let needle = app_name_needle(app_name_or_bundle_identifier);
    let dbus = DBusProxy::new(conn.connection()).await.ok();
    let mut remaining = roots;

    if let Some(target_pid) = target_pid {
        let mut pid_and_filter_matches = Vec::new();
        let mut pid_matches = Vec::new();
        let mut non_pid_matches = Vec::new();

        for object_ref in remaining {
            if object_ref_pid(dbus.as_ref(), &object_ref).await == Some(target_pid) {
                if let Some(needle) = needle.as_deref() {
                    if root_matches(conn, &object_ref, needle, remaining_child_reads).await {
                        pid_and_filter_matches.push(object_ref);
                    } else {
                        pid_matches.push(object_ref);
                    }
                } else {
                    pid_matches.push(object_ref);
                }
            } else {
                non_pid_matches.push(object_ref);
            }
        }

        if !pid_and_filter_matches.is_empty() {
            return SelectedRoots {
                roots: pid_and_filter_matches,
                scoped: true,
                pid: Some(target_pid),
            };
        }
        if !pid_matches.is_empty() {
            return SelectedRoots {
                roots: pid_matches,
                scoped: true,
                pid: Some(target_pid),
            };
        }

        remaining = non_pid_matches;
    }

    let Some(needle) = needle.as_deref() else {
        return SelectedRoots {
            roots: remaining,
            scoped: false,
            pid: None,
        };
    };

    let mut selected = Vec::new();
    for object_ref in remaining {
        if root_matches(conn, &object_ref, needle, remaining_child_reads).await {
            selected.push(object_ref);
        }
    }

    SelectedRoots {
        roots: selected,
        scoped: true,
        pid: None,
    }
}

async fn root_matches(
    conn: &AccessibilityConnection,
    object_ref: &ObjectRefOwned,
    needle: &str,
    remaining_child_reads: &mut usize,
) -> bool {
    let Ok(proxy) = open_accessible(conn, object_ref).await else {
        return object_ref_id(object_ref)
            .to_ascii_lowercase()
            .contains(needle);
    };

    if proxy_matches(&proxy, object_ref, needle).await {
        return true;
    }

    for child_ref in children_up_to(&proxy, ROOT_MATCH_CHILD_LIMIT, remaining_child_reads)
        .await
        .map(|batch| batch.items)
        .unwrap_or_default()
    {
        let Ok(child_proxy) = open_accessible(conn, &child_ref).await else {
            continue;
        };
        if proxy_matches(&child_proxy, &child_ref, needle).await {
            return true;
        }
    }

    false
}

async fn proxy_matches(
    proxy: &AccessibleProxy<'_>,
    object_ref: &ObjectRefOwned,
    needle: &str,
) -> bool {
    let name = proxy.name().await.unwrap_or_default();
    let role = proxy.get_role_name().await.unwrap_or_default();
    format!("{} {} {}", object_ref_id(object_ref), name, role)
        .to_ascii_lowercase()
        .contains(needle)
}

async fn read_app_summary(
    proxy: &AccessibleProxy<'_>,
    object_ref: &ObjectRefOwned,
    dbus: Option<&DBusProxy<'_>>,
) -> AccessibleAppSummary {
    AccessibleAppSummary {
        object_ref: object_ref_id(object_ref),
        name: optional_string(proxy.name().await.ok()),
        pid: object_ref_pid(dbus, object_ref).await,
        role: role_name(proxy).await,
        child_count: proxy.child_count().await.unwrap_or_default(),
        bounds: bounds(proxy).await,
    }
}

async fn read_node(
    proxy: &AccessibleProxy<'_>,
    object_ref: &ObjectRefOwned,
    index: u32,
    parent_index: Option<u32>,
    depth: u32,
) -> AccessibilityNode {
    // Independent reads go out together; each is one bus round trip.
    let (proxies, role, name, description, child_count, states) = futures_util::join!(
        proxy.proxies(),
        role_name(proxy),
        proxy.name(),
        proxy.description(),
        proxy.child_count(),
        states_from_proxy(proxy),
    );
    let proxies = proxies.ok();
    let (bounds, actions, value, text, supports_editable_text) = futures_util::join!(
        bounds_from_proxies(proxies.as_ref(), proxy),
        actions_from_proxies(proxies.as_ref()),
        value_from_proxies(proxies.as_ref()),
        text_from_proxies(proxies.as_ref()),
        supports_editable_text(proxies.as_ref()),
    );

    AccessibilityNode {
        index,
        parent_index,
        depth,
        object_ref: object_ref_id(object_ref),
        role,
        name: optional_string(name.ok()),
        description: optional_string(description.ok()),
        child_count: child_count.unwrap_or_default(),
        bounds,
        states,
        actions,
        value,
        text,
        supports_editable_text,
    }
}

/// Role reported when neither AT-SPI role read succeeds.
pub(crate) const UNKNOWN_ROLE: &str = "unknown";

async fn role_name(proxy: &AccessibleProxy<'_>) -> String {
    if let Ok(role) = proxy.get_role_name().await {
        if !role.trim().is_empty() {
            return role;
        }
    }
    proxy
        .get_role()
        .await
        .map(|role| format!("{role:?}"))
        .unwrap_or_else(|_| UNKNOWN_ROLE.to_string())
}

async fn bounds(proxy: &AccessibleProxy<'_>) -> Option<Bounds> {
    bounds_from_proxies(proxy.proxies().await.ok().as_ref(), proxy).await
}

/// Pid of the process that owns an AT-SPI object ref (`:bus/path`), from the
/// accessibility bus. `None` when the owner is gone or the bus cannot say.
pub(crate) async fn object_ref_owner_pid(object_ref_id: &str) -> Result<Option<u32>> {
    let object_ref = object_ref_from_id(object_ref_id)?;
    let conn = connect().await?;
    let dbus = DBusProxy::new(conn.connection()).await.ok();
    Ok(object_ref_pid(dbus.as_ref(), &object_ref).await)
}

async fn object_ref_pid(dbus: Option<&DBusProxy<'_>>, object_ref: &ObjectRefOwned) -> Option<u32> {
    let dbus = dbus?;
    let bus_name = BusName::try_from(object_ref.name_as_str()?.to_string()).ok()?;
    dbus.get_connection_unix_process_id(bus_name).await.ok()
}

async fn bounds_from_proxies(
    proxies: Option<&atspi::proxy::proxy_ext::Proxies<'_>>,
    proxy: &AccessibleProxy<'_>,
) -> Option<Bounds> {
    let owned_proxies;
    let proxies = if let Some(proxies) = proxies {
        proxies
    } else {
        owned_proxies = proxy.proxies().await.ok()?;
        &owned_proxies
    };
    let component = proxies.component().await.ok()?;
    let (x, y, width, height) = component.get_extents(CoordType::Screen).await.ok()?;
    normalize_bounds(Bounds {
        x,
        y,
        width,
        height,
    })
}

fn normalize_bounds(bounds: Bounds) -> Option<Bounds> {
    if bounds.width <= 0 || bounds.height <= 0 {
        return None;
    }
    if bounds.x <= i32::MIN / 2 || bounds.y <= i32::MIN / 2 {
        return None;
    }
    Some(bounds)
}

async fn actions_from_proxies(
    proxies: Option<&atspi::proxy::proxy_ext::Proxies<'_>>,
) -> Vec<AccessibilityAction> {
    let Some(proxies) = proxies else {
        return Vec::new();
    };
    let Ok(action_proxy) = proxies.action().await else {
        return Vec::new();
    };

    let mut actions: Vec<AccessibilityAction> = action_proxy
        .get_actions()
        .await
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, action)| AccessibilityAction {
            index: index as i32,
            name: action.name,
            description: action.description,
            keybinding: action.keybinding,
        })
        .collect();
    // Chromium answers GetActions with blank names but GetName(i) with the
    // real ones ("click", "doDefault"), so fill the gaps per index.
    let names = futures_util::future::join_all(
        actions
            .iter()
            .filter(|action| action.name.is_empty())
            .map(|action| action_proxy.get_name(action.index)),
    )
    .await;
    for (action, name) in actions
        .iter_mut()
        .filter(|action| action.name.is_empty())
        .zip(names)
    {
        if let Ok(name) = name {
            action.name = name;
        }
    }
    actions
}

async fn states_from_proxy(proxy: &AccessibleProxy<'_>) -> Vec<String> {
    proxy
        .get_state()
        .await
        .map(state_labels)
        .unwrap_or_default()
}

async fn value_from_proxies(
    proxies: Option<&atspi::proxy::proxy_ext::Proxies<'_>>,
) -> Option<AccessibilityValue> {
    let value = proxies?.value().await.ok()?;
    Some(AccessibilityValue {
        current: value.current_value().await.ok()?,
        minimum: value.minimum_value().await.ok()?,
        maximum: value.maximum_value().await.ok()?,
        minimum_increment: value.minimum_increment().await.ok()?,
        text: optional_string(value.text().await.ok()),
    })
}

async fn text_from_proxies(
    proxies: Option<&atspi::proxy::proxy_ext::Proxies<'_>>,
) -> Option<AccessibilityText> {
    let text = proxies?.text().await.ok()?;
    let (character_count, caret_offset, selection_count) = futures_util::join!(
        text.character_count(),
        text.caret_offset(),
        text.get_nselections(),
    );
    let character_count = character_count.ok()?.max(0);
    let caret_offset = caret_offset.ok();
    let capped_count = character_count.min(MAX_TEXT_READBACK_CHARS);
    let content = if capped_count > 0 {
        optional_string(text.get_text(0, capped_count).await.ok())
    } else {
        None
    };
    let selection_count = selection_count
        .unwrap_or_default()
        .clamp(0, MAX_TEXT_SELECTIONS);
    let mut selections = Vec::new();
    for index in 0..selection_count {
        if let Ok((start_offset, end_offset)) = text.get_selection(index).await {
            selections.push(AccessibilityTextSelection {
                start_offset,
                end_offset,
            });
        }
    }

    Some(AccessibilityText {
        character_count,
        caret_offset,
        content,
        truncated: character_count > MAX_TEXT_READBACK_CHARS,
        selections,
    })
}

async fn supports_editable_text(proxies: Option<&atspi::proxy::proxy_ext::Proxies<'_>>) -> bool {
    let Some(proxies) = proxies else {
        return false;
    };
    proxies.editable_text().await.is_ok()
}

fn state_labels(state_set: StateSet) -> Vec<String> {
    state_set.iter().map(|state| state.to_string()).collect()
}

fn select_action_index(actions: &[atspi::Action], requested_action: Option<&str>) -> Result<i32> {
    if actions.is_empty() {
        return Err(anyhow!("element exposes no AT-SPI actions"));
    }

    if let Some(requested_action) = requested_action
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let requested_action = requested_action.to_ascii_lowercase();
        if let Some((index, _)) = actions.iter().enumerate().find(|(_, action)| {
            action.name.to_ascii_lowercase() == requested_action
                || action.description.to_ascii_lowercase() == requested_action
        }) {
            return Ok(index as i32);
        }

        if let Ok(index) = requested_action.parse::<usize>() {
            if index < actions.len() {
                return Ok(index as i32);
            }
        }

        return Err(anyhow!(
            "requested AT-SPI action was not found; available actions: {}",
            actions
                .iter()
                .map(|action| action.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    Ok(primary_action_index(actions))
}

/// Picks the action a plain activation means: the first well-known primary
/// verb, or index 0. Neither "the first" nor "the second" action is safe on
/// its own, since Chromium lists `showContextMenu` beside its primary verb.
fn primary_action_index(actions: &[atspi::Action]) -> i32 {
    const PRIMARY: [&str; 7] = [
        "click",
        "press",
        "activate",
        "dodefault",
        "jump",
        "toggle",
        "open",
    ];
    actions
        .iter()
        .position(|action| {
            let name = action.name.to_ascii_lowercase();
            PRIMARY.contains(&name.as_str())
        })
        .unwrap_or(0) as i32
}

fn select_named_action_index(actions: &[atspi::Action], name: &str) -> Result<i32> {
    actions
        .iter()
        .position(|action| action.name.eq_ignore_ascii_case(name))
        .map(|index| index as i32)
        .ok_or_else(|| {
            anyhow!("cached AT-SPI action name is no longer available; refresh get_app_state")
        })
}

fn optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn object_ref_from_id(object_ref_id: &str) -> Result<ObjectRefOwned> {
    let (name, path) = split_object_ref_id(object_ref_id)?;
    let name = UniqueName::try_from(name.to_string())
        .with_context(|| format!("invalid AT-SPI bus name in object ref {object_ref_id}"))?;
    let path = ObjectPath::try_from(path.to_string())
        .with_context(|| format!("invalid AT-SPI object path in object ref {object_ref_id}"))?;
    Ok(ObjectRef::new_owned(name, path))
}

fn split_object_ref_id(object_ref_id: &str) -> Result<(&str, &str)> {
    let Some(path_start) = object_ref_id.find('/') else {
        return Err(anyhow!(
            "invalid AT-SPI object ref '{object_ref_id}'; expected ':bus/path'"
        ));
    };
    let (name, path) = object_ref_id.split_at(path_start);
    if name.is_empty() || path.is_empty() {
        return Err(anyhow!(
            "invalid AT-SPI object ref '{object_ref_id}'; expected ':bus/path'"
        ));
    }
    Ok((name, path))
}

fn object_ref_id(object_ref: &ObjectRefOwned) -> String {
    format!(
        "{}{}",
        object_ref.name_as_str().unwrap_or(""),
        object_ref.path_as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_object_ref_id_separates_bus_name_and_path() {
        let (name, path) = split_object_ref_id(":1.42/org/a11y/atspi/accessible/7").unwrap();

        assert_eq!(name, ":1.42");
        assert_eq!(path, "/org/a11y/atspi/accessible/7");
    }

    #[test]
    fn select_action_index_uses_named_action() {
        let actions = vec![
            atspi::Action {
                name: "click".to_string(),
                description: "Clicks".to_string(),
                keybinding: String::new(),
            },
            atspi::Action {
                name: "show-menu".to_string(),
                description: "Shows menu".to_string(),
                keybinding: String::new(),
            },
        ];

        assert_eq!(select_action_index(&actions, Some("show-menu")).unwrap(), 1);
    }

    #[test]
    fn cached_action_name_is_revalidated_against_current_actions() {
        let mut actions = vec![atspi::Action {
            name: "show-menu".into(),
            description: "Click".into(),
            keybinding: String::new(),
        }];
        // Neither a stale index nor a matching description may invoke another action.
        assert!(select_named_action_index(&actions, "Click").is_err());
        actions.push(atspi::Action {
            name: "click".into(),
            description: String::new(),
            keybinding: String::new(),
        });
        assert_eq!(select_named_action_index(&actions, "Click").unwrap(), 1);
    }

    #[test]
    fn select_action_index_defaults_to_primary_verb() {
        let actions = vec![
            atspi::Action {
                name: "click".to_string(),
                description: String::new(),
                keybinding: String::new(),
            },
            atspi::Action {
                name: "show-menu".to_string(),
                description: String::new(),
                keybinding: String::new(),
            },
        ];

        assert_eq!(select_action_index(&actions, None).unwrap(), 0);

        let chromium = vec![
            atspi::Action {
                name: "showContextMenu".to_string(),
                description: String::new(),
                keybinding: String::new(),
            },
            atspi::Action {
                name: "doDefault".to_string(),
                description: String::new(),
                keybinding: String::new(),
            },
        ];
        assert_eq!(select_action_index(&chromium, None).unwrap(), 1);
    }

    #[test]
    fn state_labels_serialize_in_bit_order() {
        let labels = state_labels(StateSet::new(atspi::State::Focused | atspi::State::Checked));

        assert_eq!(labels, vec!["checked".to_string(), "focused".to_string()]);
    }

    #[test]
    fn default_snapshot_limits_cover_deep_gtk4_trees() {
        // Nautilus 50 places file-list cells below depth 20 and can expose
        // more than 850 raw nodes. Keep the defaults above that known shape.
        assert_eq!(snapshot_limits(None, None), (1_000, 32));
    }

    #[test]
    fn app_name_needle_ignores_blank_values_and_normalizes_the_rest() {
        assert_eq!(app_name_needle(None), None);
        assert_eq!(app_name_needle(Some("")), None);
        assert_eq!(app_name_needle(Some("   ")), None);
        assert_eq!(
            app_name_needle(Some("Calculator")).as_deref(),
            Some("calculator")
        );
        assert_eq!(
            app_name_needle(Some(" :1.64/org/a11y/atspi/accessible/root ")).as_deref(),
            Some(":1.64/org/a11y/atspi/accessible/root")
        );
    }

    #[test]
    fn requested_snapshot_limits_remain_bounded() {
        assert_eq!(snapshot_limits(Some(0), Some(0)), (1, 0));
        assert_eq!(snapshot_limits(Some(10_000), Some(128)), (2_000, 64));
    }

    #[test]
    fn app_discovery_budget_is_independent_of_requested_tree_size() {
        assert_eq!(snapshot_child_read_budgets(1), (256, 2_048, 1));
    }

    #[test]
    fn traversal_attempts_and_queue_share_one_work_budget() {
        let mut traversal = BoundedTraversal::new(4);
        traversal.enqueue([1]);

        assert_eq!(traversal.pop(), Some(1));
        traversal.enqueue(2..=10_000);
        assert_eq!(traversal.queue, VecDeque::from([2, 3, 4]));

        assert_eq!(traversal.pop(), Some(2));
        traversal.enqueue(5..=10_000);
        assert_eq!(traversal.queue, VecDeque::from([3, 4]));
        assert_eq!(traversal.pop(), Some(3));
        assert_eq!(traversal.pop(), Some(4));
        assert_eq!(traversal.pop(), None);
        assert_eq!(traversal.attempted, 4);
        assert!(
            traversal.truncated(),
            "dropped enqueue items must be reported"
        );
    }

    #[test]
    fn traversal_that_drains_within_budget_is_not_truncated() {
        let mut traversal = BoundedTraversal::new(3);
        traversal.enqueue([1, 2, 3]);
        assert_eq!(traversal.pop(), Some(1));
        assert_eq!(traversal.pop(), Some(2));
        assert_eq!(traversal.pop(), Some(3));
        assert_eq!(traversal.pop(), None);
        assert!(
            !traversal.truncated(),
            "exactly max_items real nodes is complete"
        );
    }

    #[test]
    fn enqueue_at_zero_capacity_marks_the_traversal_truncated() {
        let mut traversal = BoundedTraversal::new(2);
        traversal.enqueue([1, 2]);
        assert_eq!(traversal.pop(), Some(1));
        assert_eq!(traversal.pop(), Some(2));
        assert!(!traversal.truncated(), "nothing dropped yet");
        // Capacity is zero; the offered item is dropped, which is the only
        // path that can mark truncation.
        traversal.enqueue([3]);
        assert!(traversal.queue.is_empty());
        assert_eq!(traversal.pop(), None);
        assert!(traversal.truncated());
    }

    #[test]
    fn child_count_is_clamped_before_indexed_reads() {
        assert_eq!(bounded_child_count(-1, 4), 0);
        assert_eq!(bounded_child_count(3, 4), 3);
        assert_eq!(bounded_child_count(i32::MAX, 4), 4);
    }

    #[tokio::test]
    async fn indexed_child_reads_consume_attempts_even_when_one_fails() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut remaining_attempts = 3;
        let batch = fetch_indexed_up_to(100, 10, &mut remaining_attempts, {
            let calls = calls.clone();
            move |index| {
                let calls = calls.clone();
                async move {
                    calls.lock().unwrap().push(index);
                    if index == 1 {
                        Err(())
                    } else {
                        Ok(index)
                    }
                }
            }
        })
        .await;

        assert_eq!(batch.items, vec![0, 2]);
        assert_eq!(batch.attempted, 3);
        assert!(
            batch.incomplete,
            "100 reported children, 3 attempted: budget cut must be reported"
        );
        assert!(!batch.all_failed());
        assert_eq!(*calls.lock().unwrap(), vec![0, 1, 2]);
        assert_eq!(remaining_attempts, 0);

        let no_children = fetch_indexed_up_to(100, 10, &mut remaining_attempts, |_| async {
            Ok::<_, ()>(99)
        })
        .await;
        assert!(no_children.items.is_empty());
        assert_eq!(no_children.attempted, 0);
        assert!(!no_children.all_failed());

        let mut failed_attempts = 2;
        let all_failed =
            fetch_indexed_up_to(2, 2, &mut failed_attempts, |_| async { Err::<i32, _>(()) }).await;
        assert!(all_failed.all_failed());
    }
}
