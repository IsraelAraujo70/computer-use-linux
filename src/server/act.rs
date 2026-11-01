//! `act`: several steps in one call.
//!
//! Every model turn costs an agent seconds, far more than the server spends
//! on a step, so a task that needs click, fill, click should not need three
//! turns. `act` runs the steps in order, lets the page settle after each one
//! (so the next step finds elements the previous one revealed), and stops at
//! the first step that fails or when a dialog it did not ask for opens. The
//! result is one line per step that ran, then one change report against the
//! page the agent saw before the call.
//!
//! Steps name elements by id or by role/name/text, so a step may target an
//! element that only appears after an earlier step ("click Responder, then
//! set the reply box"). Such a selector is retried for a short while before
//! the step fails, and a failed step says what was not found.

use super::{ElementQuery, WaitCondition};
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Most steps one `act` call takes.
pub(super) const MAX_STEPS: usize = 25;
/// How long a step whose element is named by role/name/text waits for that
/// element to appear (the previous step may still be rendering it).
pub(super) const STEP_FIND_LIMIT: Duration = Duration::from_millis(2_500);
/// Default and longest wait of a `wait` step.
pub(super) const WAIT_STEP_DEFAULT: Duration = Duration::from_secs(5);
pub(super) const WAIT_STEP_MAX: Duration = Duration::from_secs(30);

/// What a step does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum StepKind {
    /// Click the element.
    Click,
    /// Set a field like set_value: text, option name, "true"/"false".
    Set,
    /// Type text (into the element when one is named, else into focus).
    Type,
    /// Press a key or chord, e.g. Enter, Escape, Ctrl+L.
    Key,
    /// Scroll the element into view, or scroll until role/name/text appears.
    Scroll,
    /// Wait until role/name/text appears (or disappears, or anything changes).
    Wait,
}

/// One step of `act`, as the agent writes it: a flat object whose `do`
/// says what to do and whose other fields say to what.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub(super) struct ActStep {
    /// click, set, type, key, scroll or wait.
    #[serde(rename = "do")]
    pub kind: StepKind,
    /// Element id `[n]` from an earlier result.
    #[serde(default)]
    pub element_index: Option<u32>,
    /// Role substring (button, entry, check box, link, …), to name an
    /// element without an id, e.g. one an earlier step reveals.
    #[serde(default)]
    pub role: Option<String>,
    /// Name substring.
    #[serde(default)]
    pub name: Option<String>,
    /// Substring of the element's name, text or value. For `type` with no
    /// `value`, the text to type.
    #[serde(default)]
    pub text: Option<String>,
    /// set: the value (text, option name, "true"/"false"). type: the text.
    /// key: the key or chord.
    #[serde(default)]
    pub value: Option<String>,
    /// key: the key or chord (same as value).
    #[serde(default)]
    pub key: Option<String>,
    /// scroll: up or down; with role/name/text and no direction it searches
    /// down, then up.
    #[serde(default)]
    pub direction: Option<String>,
    /// wait: appears (default with role/name/text), disappears, or changed.
    #[serde(default)]
    pub condition: Option<WaitCondition>,
    /// wait: give up after this many milliseconds (default 5000, max 30000).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// click: 2 for a double click.
    #[serde(default)]
    pub click_count: Option<u32>,
}

/// Parameters of `act`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub(super) struct ActParams {
    /// Steps, run in order; the call stops at the first that fails.
    pub steps: Vec<ActStep>,
}

/// The element a step acts on: an id, or a role/name/text selector.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct StepTarget {
    pub element_index: Option<u32>,
    pub role: Option<String>,
    pub name: Option<String>,
    pub text: Option<String>,
}

/// A step after validation: each kind with exactly what it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StepPlan {
    Click {
        target: StepTarget,
        click_count: u32,
    },
    Set {
        target: StepTarget,
        value: String,
    },
    Type {
        target: Option<StepTarget>,
        text: String,
    },
    Key {
        key: String,
    },
    /// `target` brings an element by id into view; `until` scrolls until a
    /// match is rendered; neither scrolls the page one step in `direction`.
    Scroll {
        target: Option<u32>,
        until: Option<ElementQuery>,
        direction: Option<String>,
    },
    Wait {
        query: ElementQuery,
        condition: WaitCondition,
        timeout: Duration,
    },
}

/// Checks one step and turns it into its plan, or says what is missing in
/// words the agent can act on (`step needs …`). Blank strings count as
/// absent. `type` takes its text from `value`, else from `text` (then
/// `text` is not a selector); `key` from `key`, else `value`.
pub(super) fn plan_step(step: &ActStep) -> Result<StepPlan, String> {
    let present = |s: &Option<String>| s.clone().filter(|s| !s.trim().is_empty());
    let mut target = StepTarget {
        element_index: step.element_index,
        role: present(&step.role),
        name: present(&step.name),
        text: present(&step.text),
    };
    let given = |t: &StepTarget| {
        t.element_index.is_some() || t.role.is_some() || t.name.is_some() || t.text.is_some()
    };
    let query = ElementQuery {
        role: target.role.clone(),
        name: target.name.clone(),
        text: target.text.clone(),
        states: None,
    };
    let has_query = query.role.is_some() || query.name.is_some() || query.text.is_some();
    Ok(match step.kind {
        StepKind::Click => {
            if !given(&target) {
                return Err("click needs element_index or role/name/text".into());
            }
            StepPlan::Click {
                target,
                click_count: step.click_count.unwrap_or(1).clamp(1, 3),
            }
        }
        StepKind::Set => {
            if !given(&target) {
                return Err("set needs element_index or role/name/text".into());
            }
            StepPlan::Set {
                target,
                value: step.value.clone().ok_or("set needs value")?,
            }
        }
        StepKind::Type => {
            let text = if let Some(value) = step.value.as_ref().filter(|v| !v.is_empty()) {
                value.clone()
            } else {
                target
                    .text
                    .take()
                    .ok_or("type needs value (the text to type)")?
            };
            StepPlan::Type {
                target: given(&target).then_some(target),
                text,
            }
        }
        StepKind::Key => StepPlan::Key {
            key: present(&step.key)
                .or_else(|| present(&step.value))
                .ok_or("key needs key, e.g. Enter")?,
        },
        StepKind::Scroll => {
            let direction = present(&step.direction).map(|d| d.to_lowercase());
            if direction
                .as_deref()
                .is_some_and(|d| !matches!(d, "up" | "down" | "left" | "right"))
            {
                return Err("scroll direction must be up, down, left or right".into());
            }
            StepPlan::Scroll {
                target: step.element_index,
                until: has_query.then_some(query),
                direction,
            }
        }
        StepKind::Wait => {
            let condition = step.condition.unwrap_or(if has_query {
                WaitCondition::Appears
            } else {
                WaitCondition::Changed
            });
            if !has_query && !matches!(condition, WaitCondition::Changed) {
                return Err("wait needs role/name/text to wait for (or condition changed)".into());
            }
            StepPlan::Wait {
                query,
                condition,
                timeout: step
                    .timeout_ms
                    .map(Duration::from_millis)
                    .unwrap_or(WAIT_STEP_DEFAULT)
                    .clamp(Duration::from_millis(100), WAIT_STEP_MAX),
            }
        }
    })
}

/// Checks every step before any runs, so a malformed call changes nothing.
/// Errors name the step by its 1-based number: `step 3: …`.
pub(super) fn plan_steps(steps: &[ActStep]) -> Result<Vec<StepPlan>, String> {
    if steps.is_empty() {
        return Err("act needs at least one step".into());
    }
    if steps.len() > MAX_STEPS {
        return Err(format!(
            "act takes at most {MAX_STEPS} steps; split the rest into another call"
        ));
    }
    steps
        .iter()
        .enumerate()
        .map(|(i, step)| plan_step(step).map_err(|e| format!("step {}: {e}", i + 1)))
        .collect()
}

/// How a step's line names it: `click [12]`, `click button "Enviar"`,
/// `set entry "Nome" = "Joana"`, `type "olá"`, `key Enter`,
/// `scroll until button "Arquivar pedido #1377"`, `wait for "Salvo"`.
/// Values longer than 60 characters are cut with "…".
pub(super) fn describe_step(plan: &StepPlan) -> String {
    fn quoted(value: &str) -> String {
        let short: String = value.chars().take(60).collect();
        format!(
            "\"{short}{}\"",
            if value.chars().count() > 60 {
                "…"
            } else {
                ""
            }
        )
    }
    fn selector(role: Option<&str>, name: Option<&str>, text: Option<&str>) -> String {
        let label = name.or(text).map(quoted);
        [role.map(str::to_owned), label]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ")
    }
    fn target(t: &StepTarget) -> String {
        t.element_index
            .map(|id| format!("[{id}]"))
            .unwrap_or_else(|| selector(t.role.as_deref(), t.name.as_deref(), t.text.as_deref()))
    }
    let query =
        |q: &ElementQuery| selector(q.role.as_deref(), q.name.as_deref(), q.text.as_deref());
    match plan {
        StepPlan::Click {
            target: t,
            click_count,
        } => format!(
            "{} {}",
            match click_count {
                2 => "double-click",
                3 => "triple-click",
                _ => "click",
            },
            target(t)
        ),
        StepPlan::Set { target: t, value } => format!("set {} = {}", target(t), quoted(value)),
        StepPlan::Type { target: t, text } => format!(
            "type {}{}",
            quoted(text),
            t.as_ref()
                .map(|t| format!(" into {}", target(t)))
                .unwrap_or_default()
        ),
        StepPlan::Key { key } => format!("key {key}"),
        StepPlan::Scroll {
            target: Some(id), ..
        } => format!("scroll [{id}] into view"),
        StepPlan::Scroll {
            until: Some(q),
            direction,
            ..
        } => format!(
            "scroll{} until {}",
            direction
                .as_ref()
                .map(|d| format!(" {d}"))
                .unwrap_or_default(),
            query(q)
        ),
        StepPlan::Scroll { direction, .. } => {
            format!("scroll {}", direction.as_deref().unwrap_or("down"))
        }
        StepPlan::Wait {
            query: q,
            condition,
            ..
        } => match condition {
            WaitCondition::Appears => format!("wait for {}", query(q)),
            WaitCondition::Disappears => format!("wait until {} is gone", query(q)),
            WaitCondition::Changed => "wait for a change".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(json: serde_json::Value) -> ActStep {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn errors_and_batch_limits() {
        for (input, error) in [
            (
                serde_json::json!({"do":"click","name":" \n"}),
                "click needs element_index or role/name/text",
            ),
            (
                serde_json::json!({"do":"set"}),
                "set needs element_index or role/name/text",
            ),
            (
                serde_json::json!({"do":"set","element_index":1}),
                "set needs value",
            ),
            (
                serde_json::json!({"do":"type","value":"","text":" "}),
                "type needs value (the text to type)",
            ),
            (
                serde_json::json!({"do":"key","key":" ","value":" "}),
                "key needs key, e.g. Enter",
            ),
            (
                serde_json::json!({"do":"scroll","direction":" up "}),
                "scroll direction must be up, down, left or right",
            ),
            (
                serde_json::json!({"do":"wait","condition":"appears"}),
                "wait needs role/name/text to wait for (or condition changed)",
            ),
            (
                serde_json::json!({"do":"wait","condition":"disappears"}),
                "wait needs role/name/text to wait for (or condition changed)",
            ),
        ] {
            assert_eq!(plan_step(&step(input)).unwrap_err(), error);
        }
        let good = step(serde_json::json!({"do":"key","key":"Enter"}));
        let bad = step(serde_json::json!({"do":"click"}));
        assert_eq!(plan_steps(&[]).unwrap_err(), "act needs at least one step");
        assert_eq!(
            plan_steps(&[good.clone(), bad]).unwrap_err(),
            "step 2: click needs element_index or role/name/text"
        );
        assert_eq!(
            plan_steps(&vec![good.clone(); MAX_STEPS]).unwrap().len(),
            MAX_STEPS
        );
        assert_eq!(
            plan_steps(&vec![good; MAX_STEPS + 1]).unwrap_err(),
            format!("act takes at most {MAX_STEPS} steps; split the rest into another call")
        );
    }

    #[test]
    fn descriptions_and_happy_paths() {
        for (input, expected) in [
            (
                serde_json::json!({"do":"click","element_index":12,"name":"ignored","click_count":0}),
                "click [12]",
            ),
            (
                serde_json::json!({"do":"click","element_index":12,"click_count":2}),
                "double-click [12]",
            ),
            (
                serde_json::json!({"do":"click","element_index":12,"click_count":99}),
                "triple-click [12]",
            ),
            (
                serde_json::json!({"do":"click","role":"button","name":"Enviar"}),
                "click button \"Enviar\"",
            ),
            (
                serde_json::json!({"do":"click","role":"button"}),
                "click button",
            ),
            (
                serde_json::json!({"do":"click","text":"Enviar"}),
                "click \"Enviar\"",
            ),
            (
                serde_json::json!({"do":"set","element_index":5,"value":"Joana"}),
                "set [5] = \"Joana\"",
            ),
            (
                serde_json::json!({"do":"set","element_index":5,"value":""}),
                "set [5] = \"\"",
            ),
            (
                serde_json::json!({"do":"type","text":"olá"}),
                "type \"olá\"",
            ),
            (
                serde_json::json!({"do":"type","element_index":5,"value":"olá"}),
                "type \"olá\" into [5]",
            ),
            (
                serde_json::json!({"do":"type","value":"x","text":"campo"}),
                "type \"x\" into \"campo\"",
            ),
            (
                serde_json::json!({"do":"type","value":"","text":" x "}),
                "type \" x \"",
            ),
            (
                serde_json::json!({"do":"key","key":" ","value":"Enter"}),
                "key Enter",
            ),
            (
                serde_json::json!({"do":"scroll","element_index":7}),
                "scroll [7] into view",
            ),
            (
                serde_json::json!({"do":"scroll","role":"button","name":"Arquivar pedido #1377"}),
                "scroll until button \"Arquivar pedido #1377\"",
            ),
            (
                serde_json::json!({"do":"scroll","text":"x","direction":"UP"}),
                "scroll up until \"x\"",
            ),
            (serde_json::json!({"do":"scroll"}), "scroll down"),
            (
                serde_json::json!({"do":"wait","name":"Salvo"}),
                "wait for \"Salvo\"",
            ),
            (
                serde_json::json!({"do":"wait","text":"Carregando","condition":"disappears"}),
                "wait until \"Carregando\" is gone",
            ),
            (serde_json::json!({"do":"wait"}), "wait for a change"),
        ] {
            assert_eq!(describe_step(&plan_step(&step(input)).unwrap()), expected);
        }
        let text = "á".repeat(61);
        assert_eq!(
            describe_step(&StepPlan::Type { target: None, text }),
            format!("type \"{}…\"", "á".repeat(60))
        );
    }

    #[test]
    fn wait_bounds_and_preserved_values() {
        for (timeout, expected) in [
            (None, WAIT_STEP_DEFAULT),
            (Some(0), Duration::from_millis(100)),
            (Some(u64::MAX), WAIT_STEP_MAX),
        ] {
            let mut s = step(serde_json::json!({"do":"wait"}));
            s.timeout_ms = timeout;
            assert!(
                matches!(plan_step(&s).unwrap(), StepPlan::Wait { timeout, .. } if timeout == expected)
            );
        }
        let s = step(serde_json::json!({"do":"click","role":" button ","name":" ","text":" x "}));
        assert!(
            matches!(plan_step(&s).unwrap(), StepPlan::Click { target, .. } if target.role.as_deref() == Some(" button ") && target.name.is_none() && target.text.as_deref() == Some(" x "))
        );
    }
}
