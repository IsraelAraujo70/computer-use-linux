//! Pure decisions behind `set_value`, `fill_form` and `type_text`: what kind
//! of control an element is, which option a value names, and whether the text
//! read back from a field is the text that was asked for. The AT-SPI and
//! keyboard work that uses them lives in the server.

use crate::atspi_tree::AccessibilityNode;
use crate::outline::clean;

/// How `set_value` changes an element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldKind {
    /// Check box, radio button, switch or toggle button: `"true"`/`"false"`.
    Toggle,
    /// Combo box or list box: the value names one of its options.
    Choice,
    /// Editable text: entry, text area, contenteditable section.
    Text,
    /// Anything else; only the AT-SPI Value/EditableText path can set it.
    Other,
}

/// Roles whose value is a checked state.
const TOGGLE_ROLES: [&str; 6] = [
    "check box",
    "radio button",
    "switch",
    "toggle button",
    "check menu item",
    "radio menu item",
];

/// Roles whose value is one of their options.
const CHOICE_ROLES: [&str; 2] = ["combo box", "list box"];

/// Roles Chromium and GTK give to a text field.
const TEXT_ROLES: [&str; 6] = [
    "entry",
    "text",
    "textbox",
    "password text",
    "spin button",
    "terminal",
];

/// Roles of the items a combo box or list box offers.
pub(crate) const OPTION_ROLES: [&str; 5] = [
    "option",
    "list item",
    "menu item",
    "radio menu item",
    "check menu item",
];

/// Classifies an element for `set_value`.
pub(crate) fn field_kind(node: &AccessibilityNode) -> FieldKind {
    let role = node.role.to_ascii_lowercase();
    if TOGGLE_ROLES.contains(&role.as_str()) {
        FieldKind::Toggle
    } else if CHOICE_ROLES.contains(&role.as_str()) {
        FieldKind::Choice
    } else if is_editable(node) || TEXT_ROLES.contains(&role.as_str()) {
        FieldKind::Text
    } else {
        FieldKind::Other
    }
}

/// Editable as far as AT-SPI says: the EditableText interface or the
/// `editable` state. Chromium web fields have the state but not the interface.
pub(crate) fn is_editable(node: &AccessibilityNode) -> bool {
    node.supports_editable_text || has_state(node, "editable")
}

pub(crate) fn has_state(node: &AccessibilityNode, state: &str) -> bool {
    node.states
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(state))
}

/// The checked state of a toggle; toggle buttons report `pressed` instead.
pub(crate) fn is_checked(node: &AccessibilityNode) -> bool {
    has_state(node, "checked") || has_state(node, "pressed")
}

pub(crate) fn is_option_role(role: &str) -> bool {
    OPTION_ROLES.contains(&role.to_ascii_lowercase().as_str())
}

/// The boolean a toggle value names, or `None` when it names none.
pub(crate) fn parse_toggle(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "true" | "on" | "yes" | "1" | "checked" | "check" | "sim" | "selected" => Some(true),
        "false" | "off" | "no" | "0" | "unchecked" | "uncheck" | "não" | "nao" => Some(false),
        _ => None,
    }
}

/// Picks the option a value names: a case-insensitive exact name, else the
/// only name starting with it, else the only name containing it. The error
/// lists the candidates so the caller can pass an exact name.
pub(crate) fn match_option(names: &[&str], wanted: &str) -> Result<usize, String> {
    let wanted_key = comparable(wanted);
    if wanted_key.is_empty() {
        return Err("pass the option's name as value".to_string());
    }
    let keys: Vec<String> = names.iter().map(|name| comparable(name)).collect();
    if let Some(index) = keys.iter().position(|key| *key == wanted_key) {
        return Ok(index);
    }
    for predicate in [
        (|key: &str, wanted: &str| key.starts_with(wanted)) as fn(&str, &str) -> bool,
        |key: &str, wanted: &str| key.contains(wanted),
    ] {
        let hits: Vec<usize> = keys
            .iter()
            .enumerate()
            .filter(|(_, key)| predicate(key, &wanted_key))
            .map(|(index, _)| index)
            .collect();
        match hits.as_slice() {
            [only] => return Ok(*only),
            [] => continue,
            many => {
                return Err(format!(
                    "\"{wanted}\" matches several options: {}",
                    quoted_list(many.iter().map(|&index| names[index]))
                ))
            }
        }
    }
    Err(if names.is_empty() {
        format!("no option named \"{wanted}\" is available")
    } else {
        format!(
            "no option named \"{wanted}\"; options: {}",
            quoted_list(names.iter().copied())
        )
    })
}

fn quoted_list<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let names: Vec<String> = names.map(|name| format!("\"{name}\"")).collect();
    let shown = names.len().min(12);
    let mut list = names[..shown].join(", ");
    if names.len() > shown {
        list.push_str(&format!(", … ({} more)", names.len() - shown));
    }
    list
}

/// Whitespace-collapsed, case-folded form used for every comparison here.
fn comparable(value: &str) -> String {
    clean(Some(value)).unwrap_or_default().to_lowercase()
}

/// Whether a field's read-back text is the requested value, ignoring
/// whitespace runs (and U+FFFC markers) but not case.
pub(crate) fn values_match(actual: Option<&str>, expected: &str) -> bool {
    clean(actual).unwrap_or_default() == clean(Some(expected)).unwrap_or_default()
}

/// Whether `xdotool type` can enter the text as-is. XTEST on Xvfb drops
/// characters outside printable ASCII in Chromium, so anything else goes
/// through a clipboard paste.
pub(crate) fn typeable_ascii(text: &str) -> bool {
    text.chars()
        .all(|ch| ch == '\n' || ch == '\t' || (' '..='~').contains(&ch))
}

/// The text a field currently holds, as AT-SPI reports it: the Text
/// interface's content, else the Value interface's text.
pub(crate) fn field_text(node: &AccessibilityNode) -> Option<String> {
    node.text
        .as_ref()
        .and_then(|text| text.content.clone())
        .or_else(|| node.value.as_ref().and_then(|value| value.text.clone()))
}

/// What a combo box currently shows: its text, else its value text, else
/// its name (a custom combobox button is named by its label).
pub(crate) fn choice_text(node: &AccessibilityNode) -> Option<String> {
    clean(field_text(node).as_deref()).or_else(|| clean(node.name.as_deref()))
}

/// Containers whose option descendants a collapsed combo box can open.
const OPTION_CONTAINER_ROLES: [&str; 4] = ["list box", "menu", "list", "tree"];

/// Positions of the options `set_value` may pick for the choice element at
/// `target` (a position in `nodes`), in tree order. `parent_index` values are
/// looked up through each node's `index`, so the nodes may carry positional
/// or stable indices.
///
/// The element's own option descendants win (native `<select>`, list box).
/// Otherwise the options come from showing list boxes and menus elsewhere in
/// the tree (an ARIA combobox's popup is a sibling): those absent from
/// `before` (opened just now) first, then those named like the element.
/// `radio_group` collects radio buttons instead, for a fieldset target.
pub(crate) fn option_candidates(
    nodes: &[AccessibilityNode],
    target: usize,
    before: Option<&std::collections::HashSet<String>>,
    radio_group: bool,
) -> Vec<usize> {
    let position: std::collections::HashMap<u32, usize> = nodes
        .iter()
        .enumerate()
        .map(|(position, node)| (node.index, position))
        .collect();
    let ancestors = |mut at: usize| {
        let mut chain = Vec::new();
        while let Some(parent) = nodes[at]
            .parent_index
            .and_then(|parent| position.get(&parent).copied())
        {
            if chain.contains(&parent) {
                break;
            }
            chain.push(parent);
            at = parent;
        }
        chain
    };
    let is_option = |node: &AccessibilityNode| {
        if radio_group {
            node.role.eq_ignore_ascii_case("radio button")
        } else {
            is_option_role(&node.role)
        }
    };
    let own: Vec<usize> = (0..nodes.len())
        .filter(|&at| is_option(&nodes[at]) && ancestors(at).contains(&target))
        .collect();
    if !own.is_empty() || radio_group {
        return own;
    }

    let containers: Vec<usize> = (0..nodes.len())
        .filter(|&at| {
            OPTION_CONTAINER_ROLES.contains(&nodes[at].role.to_ascii_lowercase().as_str())
                && has_state(&nodes[at], "showing")
        })
        .collect();
    let mut chosen = containers.clone();
    if let Some(before) = before {
        let fresh: Vec<usize> = chosen
            .iter()
            .copied()
            .filter(|&at| !before.contains(&nodes[at].object_ref))
            .collect();
        if !fresh.is_empty() {
            chosen = fresh;
        }
    }
    let target_name = comparable(nodes[target].name.as_deref().unwrap_or_default());
    let named: Vec<usize> = chosen
        .iter()
        .copied()
        .filter(|&at| {
            !target_name.is_empty()
                && comparable(nodes[at].name.as_deref().unwrap_or_default()) == target_name
        })
        .collect();
    if !named.is_empty() {
        chosen = named;
    }
    (0..nodes.len())
        .filter(|&at| {
            is_option(&nodes[at]) && ancestors(at).iter().any(|parent| chosen.contains(parent))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atspi_tree::AccessibilityText;

    fn tree_node(
        index: u32,
        parent: Option<u32>,
        role: &str,
        name: &str,
        states: &[&str],
    ) -> AccessibilityNode {
        let mut node = node(role, states);
        node.index = index;
        node.parent_index = parent;
        node.object_ref = format!(":1.1/{index}");
        node.name = Some(name.to_string());
        node
    }

    #[test]
    fn options_come_from_the_element_first_then_its_open_popup() {
        // Native select: menu items under the combo box itself.
        let select = vec![
            tree_node(10, None, "combo box", "Estado", &["showing"]),
            tree_node(11, Some(10), "menu", "", &[]),
            tree_node(12, Some(11), "menu item", "MG - Minas Gerais", &[]),
            tree_node(20, None, "list box", "Outra", &["showing"]),
            tree_node(21, Some(20), "list item", "Minas Gerais", &[]),
        ];
        assert_eq!(option_candidates(&select, 0, None, false), vec![2]);

        // ARIA combobox: the popup is a sibling; prefer the one opened just
        // now and named like the combo box.
        let aria = vec![
            tree_node(1, None, "combo box", "Hora de início", &["showing"]),
            tree_node(2, None, "list box", "Hora de término", &["showing"]),
            tree_node(3, Some(2), "list item", "14:30", &[]),
            tree_node(4, None, "list box", "Hora de início", &["showing"]),
            tree_node(5, Some(4), "list item", "14:30", &[]),
            tree_node(6, None, "list box", "Escondida", &[]),
            tree_node(7, Some(6), "list item", "14:30", &[]),
        ];
        assert_eq!(option_candidates(&aria, 0, None, false), vec![4]);
        let before: std::collections::HashSet<String> = [":1.1/4".to_string()].into();
        // Only the end-time list is new, so it wins over the name match.
        assert_eq!(option_candidates(&aria, 0, Some(&before), false), vec![2]);
    }

    #[test]
    fn radio_group_options_are_its_radio_buttons() {
        let group = vec![
            tree_node(1, None, "panel", "Plano", &["showing"]),
            tree_node(2, Some(1), "radio button", "Básico", &[]),
            tree_node(3, Some(1), "radio button", "Pro", &[]),
            tree_node(4, None, "radio button", "Outro", &[]),
        ];
        assert_eq!(option_candidates(&group, 0, None, true), vec![1, 2]);
    }

    fn node(role: &str, states: &[&str]) -> AccessibilityNode {
        AccessibilityNode {
            index: 0,
            parent_index: None,
            depth: 0,
            object_ref: ":1.1/0".into(),
            role: role.into(),
            name: None,
            description: None,
            child_count: 0,
            bounds: None,
            states: states.iter().map(|state| state.to_string()).collect(),
            actions: Vec::new(),
            value: None,
            text: None,
            supports_editable_text: false,
        }
    }

    #[test]
    fn chromium_web_fields_classify_without_editable_text() {
        assert_eq!(field_kind(&node("entry", &["editable"])), FieldKind::Text);
        assert_eq!(field_kind(&node("section", &["editable"])), FieldKind::Text);
        assert_eq!(field_kind(&node("section", &[])), FieldKind::Other);
        assert_eq!(field_kind(&node("check box", &[])), FieldKind::Toggle);
        assert_eq!(field_kind(&node("radio button", &[])), FieldKind::Toggle);
        assert_eq!(
            field_kind(&node("combo box", &["editable"])),
            FieldKind::Choice
        );
        assert_eq!(field_kind(&node("list box", &[])), FieldKind::Choice);
    }

    #[test]
    fn option_match_prefers_exact_then_unique_prefix_then_unique_substring() {
        let states = ["Mato Grosso", "Mato Grosso do Sul", "Minas Gerais"];
        assert_eq!(match_option(&states, "mato grosso"), Ok(0));
        assert_eq!(match_option(&states, "  MINAS   gerais "), Ok(2));
        assert_eq!(match_option(&states, "Minas"), Ok(2));
        let ufs = [
            "Selecione",
            "MA - Maranhão",
            "MG - Minas Gerais",
            "MS - Mato Grosso",
        ];
        assert_eq!(match_option(&ufs, "MG"), Ok(2));
        assert_eq!(match_option(&ufs, "minas gerais"), Ok(2));
        let error = match_option(&ufs, "M").unwrap_err();
        assert!(error.contains("several options"), "{error}");
        let error = match_option(&states, "Bahia").unwrap_err();
        assert!(error.contains("\"Minas Gerais\""), "{error}");
        assert!(match_option(&states, " ").is_err());
    }

    #[test]
    fn read_back_comparison_ignores_whitespace_runs_but_not_content() {
        assert!(values_match(
            Some("Recebido,  obrigado.\u{fffc}"),
            "Recebido, obrigado."
        ));
        assert!(values_match(None, ""));
        assert!(values_match(Some("  "), ""));
        assert!(!values_match(Some("no ao"), "não ação"));
        assert!(!values_match(
            Some("recebido, obrigado."),
            "Recebido, obrigado."
        ));
    }

    #[test]
    fn only_printable_ascii_is_typed() {
        assert!(typeable_ascii("joana.ribeiro@example.com\n"));
        assert!(!typeable_ascii("Pedido não chegou"));
        assert!(!typeable_ascii("emoji 🙂"));
    }

    #[test]
    fn toggle_values_and_states() {
        assert_eq!(parse_toggle(" TRUE "), Some(true));
        assert_eq!(parse_toggle("off"), Some(false));
        assert_eq!(parse_toggle("maybe"), None);
        assert!(is_checked(&node("check box", &["checked"])));
        assert!(is_checked(&node("toggle button", &["pressed"])));
        assert!(!is_checked(&node("check box", &["focused"])));
    }

    #[test]
    fn field_text_prefers_text_content() {
        let mut entry = node("entry", &["editable"]);
        assert_eq!(field_text(&entry), None);
        entry.text = Some(AccessibilityText {
            character_count: 3,
            caret_offset: None,
            content: Some("abc".into()),
            truncated: false,
            selections: Vec::new(),
        });
        assert_eq!(field_text(&entry).as_deref(), Some("abc"));
        let mut combo = node("combo box", &[]);
        combo.name = Some("Minas Gerais".into());
        assert_eq!(choice_text(&combo).as_deref(), Some("Minas Gerais"));
    }
}
