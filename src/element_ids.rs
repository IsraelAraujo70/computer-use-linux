//! Stable element ids. Every AT-SPI object the server reports gets one id for
//! the server's lifetime, so an `[n]` printed by any earlier result still names
//! the same element after the page changes and the tree is read again.

use crate::atspi_tree::AccessibilityNode;
use std::collections::HashMap;

/// Ids keyed by AT-SPI object ref (`:bus/path`), from a counter that never
/// goes back. An object that disappears keeps its id; a new object never
/// reuses one.
#[derive(Debug)]
pub(crate) struct ElementIds {
    by_ref: HashMap<String, u32>,
    refs: HashMap<u32, String>,
    next: u32,
}

impl Default for ElementIds {
    fn default() -> Self {
        Self {
            by_ref: HashMap::new(),
            refs: HashMap::new(),
            next: 1,
        }
    }
}

impl ElementIds {
    /// The id of `object_ref`, assigning the next one on first sight.
    pub(crate) fn id_for(&mut self, object_ref: &str) -> u32 {
        if let Some(&id) = self.by_ref.get(object_ref) {
            return id;
        }
        let id = self.next;
        self.next = self.next.saturating_add(1);
        self.by_ref.insert(object_ref.to_string(), id);
        self.refs.insert(id, object_ref.to_string());
        id
    }

    /// The object an id was assigned to, whether or not it still exists.
    pub(crate) fn object_ref(&self, id: u32) -> Option<&str> {
        self.refs.get(&id).map(String::as_str)
    }

    /// Rewrites a snapshot whose `index`/`parent_index` are positions in
    /// `nodes` so that both carry stable ids instead. An object listed twice
    /// (Chromium parents some bubbles under both the app and its window)
    /// keeps only its first occurrence, so every id names one node.
    pub(crate) fn assign(&mut self, nodes: &mut Vec<AccessibilityNode>) {
        let ids: Vec<u32> = nodes
            .iter()
            .map(|node| self.id_for(&node.object_ref))
            .collect();
        let mut seen = std::collections::HashSet::new();
        let mut position = 0;
        nodes.retain_mut(|node| {
            let id = ids[position];
            position += 1;
            node.index = id;
            node.parent_index = node
                .parent_index
                .and_then(|parent| ids.get(parent as usize).copied());
            seen.insert(id)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(position: u32, parent: Option<u32>, object_ref: &str) -> AccessibilityNode {
        AccessibilityNode {
            index: position,
            parent_index: parent,
            depth: 0,
            object_ref: object_ref.to_string(),
            role: "push button".to_string(),
            name: None,
            description: None,
            child_count: 0,
            bounds: None,
            states: Vec::new(),
            actions: Vec::new(),
            value: None,
            text: None,
            supports_editable_text: false,
        }
    }

    #[test]
    fn ids_survive_snapshots_that_insert_and_remove_elements() {
        let mut ids = ElementIds::default();
        let mut first = vec![
            node(0, None, ":1.5/root"),
            node(1, Some(0), ":1.5/a"),
            node(2, Some(0), ":1.5/b"),
        ];
        ids.assign(&mut first);
        assert_eq!(
            first.iter().map(|node| node.index).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(first[2].parent_index, Some(1));

        // An element appears before `b` and `a` disappears: positions shift,
        // ids do not, and the newcomer gets a fresh id.
        let mut second = vec![
            node(0, None, ":1.5/root"),
            node(1, Some(0), ":1.5/new"),
            node(2, Some(1), ":1.5/b"),
        ];
        ids.assign(&mut second);
        assert_eq!(
            second.iter().map(|node| node.index).collect::<Vec<_>>(),
            [1, 4, 3]
        );
        assert_eq!(second[2].parent_index, Some(4));

        // A vanished element keeps its object ref for the "no longer exists"
        // check, and returning objects keep their old id.
        assert_eq!(ids.object_ref(2), Some(":1.5/a"));
        assert_eq!(ids.id_for(":1.5/a"), 2);
        assert_eq!(ids.object_ref(99), None);
    }

    #[test]
    fn an_object_listed_twice_keeps_its_first_occurrence() {
        let mut ids = ElementIds::default();
        let mut nodes = vec![
            node(0, None, ":1.5/app"),
            node(1, Some(0), ":1.5/alert"),
            node(2, Some(0), ":1.5/frame"),
            node(3, Some(2), ":1.5/alert"),
            node(4, Some(1), ":1.5/close"),
        ];
        ids.assign(&mut nodes);
        assert_eq!(
            nodes
                .iter()
                .map(|node| (node.index, node.parent_index))
                .collect::<Vec<_>>(),
            [(1, None), (2, Some(1)), (3, Some(1)), (4, Some(2))]
        );
    }
}
