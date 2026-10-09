use std::collections::HashMap;

use serde::Deserialize;
use zbus::proxy;
use zbus::zvariant::{OwnedValue, Type, Value};

pub const MAX_DEPTH: usize = 6;
pub const MAX_ITEMS: usize = 200;

pub const WANTED_PROPERTIES: &[&str] = &[
    "label",
    "enabled",
    "visible",
    "type",
    "toggle-type",
    "toggle-state",
    "children-display",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Checkmark(bool),
    Radio(bool),
}

#[derive(Debug, Clone)]
pub struct MenuNode {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub separator: bool,
    pub toggle: Option<Toggle>,
    pub submenu: bool,
    pub children: Vec<MenuNode>,
}

impl Default for MenuNode {
    fn default() -> Self {
        Self {
            id: 0,
            label: String::new(),
            enabled: true,
            separator: false,
            toggle: None,
            submenu: false,
            children: Vec::new(),
        }
    }
}

impl MenuNode {
    pub fn selectable(&self) -> bool {
        !self.separator && self.enabled
    }
}

#[derive(Debug, Deserialize, Type)]
pub struct RawNode(pub i32, pub HashMap<String, OwnedValue>, pub Vec<OwnedValue>);

#[proxy(interface = "com.canonical.dbusmenu", assume_defaults = false)]
pub trait DBusMenu {
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: &[&str],
    ) -> zbus::Result<(u32, RawNode)>;

    fn event(
        &self,
        id: i32,
        event_id: &str,
        data: &Value<'_>,
        timestamp: u32,
    ) -> zbus::Result<()>;

    fn about_to_show(&self, id: i32) -> zbus::Result<bool>;

    #[zbus(signal)]
    fn layout_updated(&self, revision: u32, parent: i32) -> zbus::Result<()>;

    #[zbus(signal)]
    fn items_properties_updated(&self) -> zbus::Result<()>;
}

/// `_` marks the mnemonic and `__` is a literal underscore; neither is drawn.
pub fn strip_mnemonics(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '_' {
            out.push(ch);
            continue;
        }
        if chars.peek() == Some(&'_') {
            chars.next();
            out.push('_');
        }
    }
    out
}

/// Property dictionaries are `a{sv}`, so every value arrives boxed in a variant.
fn unwrap<'a>(value: &'a Value<'a>) -> &'a Value<'a> {
    match value {
        Value::Value(inner) => unwrap(inner),
        other => other,
    }
}

fn as_bool(value: &Value<'_>) -> Option<bool> {
    match unwrap(value) {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

fn as_str(value: &Value<'_>) -> Option<String> {
    match unwrap(value) {
        Value::Str(s) => Some(s.to_string()),
        _ => None,
    }
}

fn as_i32(value: &Value<'_>) -> Option<i32> {
    match unwrap(value) {
        Value::I32(v) => Some(*v),
        Value::U32(v) => Some(*v as i32),
        Value::I16(v) => Some(*v as i32),
        Value::I64(v) => i32::try_from(*v).ok(),
        _ => None,
    }
}

fn node_from_parts(
    id: i32,
    properties: &HashMap<String, OwnedValue>,
    children: Vec<MenuNode>,
) -> MenuNode {
    let get = |key: &str| properties.get(key).map(|v| Value::from(v.clone()));
    let kind = get("type").and_then(|v| as_str(&v)).unwrap_or_default();
    let toggle_type = get("toggle-type").and_then(|v| as_str(&v)).unwrap_or_default();
    // -1 means indeterminate, which reads better as unchecked than as checked.
    let toggle_state = get("toggle-state").and_then(|v| as_i32(&v)).unwrap_or(-1);

    MenuNode {
        id,
        label: crate::text::sanitize(&strip_mnemonics(
            &get("label").and_then(|v| as_str(&v)).unwrap_or_default(),
        )),
        enabled: get("enabled").and_then(|v| as_bool(&v)).unwrap_or(true),
        separator: kind == "separator",
        toggle: match toggle_type.as_str() {
            "checkmark" => Some(Toggle::Checkmark(toggle_state == 1)),
            "radio" => Some(Toggle::Radio(toggle_state == 1)),
            _ => None,
        },
        submenu: get("children-display")
            .and_then(|v| as_str(&v))
            .map(|d| d == "submenu")
            .unwrap_or(false)
            || !children.is_empty(),
        children,
    }
}

/// Applications supply this tree, so depth and item count are both capped.
pub fn parse(raw: &RawNode) -> MenuNode {
    let mut budget = MAX_ITEMS;
    let mut root = parse_raw(raw, 0, &mut budget);
    tidy(&mut root);
    root
}

/// Drops leading, trailing and repeated separators, which applications emit
/// freely because a hidden item between two of them leaves a double rule.
fn tidy(node: &mut MenuNode) {
    let mut kept: Vec<MenuNode> = Vec::with_capacity(node.children.len());
    for mut child in node.children.drain(..) {
        if child.separator {
            if kept.last().map(|prev| prev.separator).unwrap_or(true) {
                continue;
            }
            kept.push(child);
            continue;
        }
        tidy(&mut child);
        kept.push(child);
    }
    while kept.last().map(|last| last.separator).unwrap_or(false) {
        kept.pop();
    }
    node.children = kept;
}

fn parse_raw(raw: &RawNode, depth: usize, budget: &mut usize) -> MenuNode {
    let children = if depth >= MAX_DEPTH {
        Vec::new()
    } else {
        raw.2
            .iter()
            .filter_map(|child| parse_value(child, depth + 1, budget))
            .collect()
    };
    node_from_parts(raw.0, &raw.1, children)
}

fn parse_value(value: &OwnedValue, depth: usize, budget: &mut usize) -> Option<MenuNode> {
    if *budget == 0 {
        return None;
    }
    *budget -= 1;

    let value = Value::from(value.clone());
    let Value::Structure(structure) = unwrap(&value) else {
        return None;
    };
    let fields = structure.fields();
    if fields.len() < 3 {
        return None;
    }
    let id = as_i32(&fields[0])?;

    let mut properties: HashMap<String, OwnedValue> = HashMap::new();
    if let Value::Dict(dict) = unwrap(&fields[1]) {
        for (key, value) in dict.iter() {
            if let (Some(key), Ok(value)) = (as_str(key), OwnedValue::try_from(value.clone())) {
                properties.insert(key, value);
            }
        }
    }

    let mut children = Vec::new();
    if depth < MAX_DEPTH {
        if let Value::Array(array) = unwrap(&fields[2]) {
            for child in array.iter() {
                let Ok(child) = OwnedValue::try_from(child.clone()) else {
                    continue;
                };
                if let Some(node) = parse_value(&child, depth + 1, budget) {
                    children.push(node);
                }
            }
        }
    }

    // A hidden item is dropped outright rather than drawn disabled.
    let visible = properties
        .get("visible")
        .map(|v| as_bool(&Value::from(v.clone())).unwrap_or(true))
        .unwrap_or(true);
    if !visible {
        return None;
    }

    Some(node_from_parts(id, &properties, children))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(pairs: &[(&str, Value<'static>)]) -> HashMap<String, OwnedValue> {
        pairs
            .iter()
            .filter_map(|(k, v)| {
                OwnedValue::try_from(v.clone())
                    .ok()
                    .map(|v| (k.to_string(), v))
            })
            .collect()
    }

    fn tree(children: Vec<MenuNode>) -> MenuNode {
        MenuNode {
            children,
            ..Default::default()
        }
    }

    fn sep() -> MenuNode {
        MenuNode {
            separator: true,
            ..Default::default()
        }
    }

    fn labelled(name: &str) -> MenuNode {
        MenuNode {
            label: name.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn leading_and_trailing_separators_are_dropped() {
        let mut node = tree(vec![sep(), labelled("One"), sep()]);
        tidy(&mut node);
        assert_eq!(node.children.len(), 1);
        assert_eq!(node.children[0].label, "One");
    }

    #[test]
    fn repeated_separators_collapse_to_one() {
        let mut node = tree(vec![labelled("One"), sep(), sep(), sep(), labelled("Two")]);
        tidy(&mut node);
        let kinds: Vec<bool> = node.children.iter().map(|c| c.separator).collect();
        assert_eq!(kinds, vec![false, true, false]);
    }

    #[test]
    fn a_menu_of_only_separators_ends_up_empty() {
        let mut node = tree(vec![sep(), sep()]);
        tidy(&mut node);
        assert!(node.children.is_empty());
    }

    #[test]
    fn separators_are_tidied_inside_submenus_too() {
        let mut node = tree(vec![tree(vec![sep(), labelled("Deep"), sep()])]);
        tidy(&mut node);
        assert_eq!(node.children[0].children.len(), 1);
        assert_eq!(node.children[0].children[0].label, "Deep");
    }

    #[test]
    fn a_mnemonic_underscore_is_removed_but_a_doubled_one_survives() {
        assert_eq!(strip_mnemonics("_Quit"), "Quit");
        assert_eq!(strip_mnemonics("E_xit"), "Exit");
        assert_eq!(strip_mnemonics("Wi__Fi"), "Wi_Fi");
        assert_eq!(strip_mnemonics("no mnemonic"), "no mnemonic");
        assert_eq!(strip_mnemonics(""), "");
    }

    #[test]
    fn a_trailing_underscore_does_not_panic() {
        assert_eq!(strip_mnemonics("Quit_"), "Quit");
    }

    #[test]
    fn defaults_apply_when_an_application_sends_no_properties() {
        let node = node_from_parts(7, &props(&[]), Vec::new());
        assert_eq!(node.id, 7);
        assert!(node.enabled, "items default to enabled per the spec");
        assert!(!node.separator);
        assert!(node.toggle.is_none());
        assert!(!node.submenu);
    }

    #[test]
    fn a_separator_is_recognised_and_is_not_selectable() {
        let node = node_from_parts(1, &props(&[("type", Value::from("separator"))]), Vec::new());
        assert!(node.separator);
        assert!(!node.selectable());
    }

    #[test]
    fn a_disabled_item_is_not_selectable() {
        let node = node_from_parts(1, &props(&[("enabled", Value::from(false))]), Vec::new());
        assert!(!node.enabled);
        assert!(!node.selectable());
    }

    #[test]
    fn checkmarks_and_radios_carry_their_state() {
        let checked = node_from_parts(
            1,
            &props(&[
                ("toggle-type", Value::from("checkmark")),
                ("toggle-state", Value::from(1i32)),
            ]),
            Vec::new(),
        );
        assert_eq!(checked.toggle, Some(Toggle::Checkmark(true)));

        let radio = node_from_parts(
            2,
            &props(&[
                ("toggle-type", Value::from("radio")),
                ("toggle-state", Value::from(0i32)),
            ]),
            Vec::new(),
        );
        assert_eq!(radio.toggle, Some(Toggle::Radio(false)));
    }

    #[test]
    fn an_indeterminate_toggle_reads_as_unchecked() {
        let node = node_from_parts(
            1,
            &props(&[
                ("toggle-type", Value::from("checkmark")),
                ("toggle-state", Value::from(-1i32)),
            ]),
            Vec::new(),
        );
        assert_eq!(node.toggle, Some(Toggle::Checkmark(false)));
    }

    #[test]
    fn a_label_is_stripped_of_mnemonics_and_sanitized() {
        let node = node_from_parts(
            1,
            &props(&[("label", Value::from("_Open\nSettings"))]),
            Vec::new(),
        );
        assert_eq!(node.label, "Open Settings");
    }

    #[test]
    fn having_children_implies_a_submenu_even_without_the_property() {
        let child = node_from_parts(2, &props(&[]), Vec::new());
        let parent = node_from_parts(1, &props(&[]), vec![child]);
        assert!(parent.submenu);
        assert_eq!(parent.children.len(), 1);
    }

    #[test]
    fn children_display_marks_a_submenu_that_has_not_been_fetched_yet() {
        let node = node_from_parts(
            1,
            &props(&[("children-display", Value::from("submenu"))]),
            Vec::new(),
        );
        assert!(node.submenu, "the arrow must show before the children arrive");
    }
}
