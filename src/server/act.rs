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
    let _ = step;
    Err("act is not implemented yet".to_string())
}

/// Checks every step before any runs, so a malformed call changes nothing.
/// Errors name the step by its 1-based number: `step 3: …`.
pub(super) fn plan_steps(steps: &[ActStep]) -> Result<Vec<StepPlan>, String> {
    let _ = steps;
    Err("act is not implemented yet".to_string())
}

/// How a step's line names it: `click [12]`, `click button "Enviar"`,
/// `set entry "Nome" = "Joana"`, `type "olá"`, `key Enter`,
/// `scroll until button "Arquivar pedido #1377"`, `wait for "Salvo"`.
/// Values longer than 60 characters are cut with "…".
pub(super) fn describe_step(plan: &StepPlan) -> String {
    let _ = plan;
    String::new()
}
