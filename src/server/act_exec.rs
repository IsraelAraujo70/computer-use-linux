//! Runs `act` steps against the live app: the executor half of `act`
//! (types and validation live in `act.rs`).

use super::act::{
    describe_step, plan_steps, ActParams, StepPlan, StepTarget, STEP_FIND_LIMIT,
};
use super::{
    resolve_semantic_node, ClickParams, ComputerUseLinux, ElementResolvePurpose, ElementSelector,
    PressKeyParams, Rescan, ScrollParams, SetValueParams, TypeTextParams, WaitCondition,
};
use crate::atspi_tree::AccessibilityNode;
use crate::outline::ElementQuery;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{CallToolResult, ContentBlock};
use std::time::{Duration, Instant};
use tokio::time::sleep;

/// Gap between reads while a step waits for its element or condition.
const STEP_POLL: Duration = Duration::from_millis(250);
/// Most `target:` lines one `act` result prints (the last elements acted on).
const MAX_TARGET_LINES: usize = 6;
/// Most matching lines a `wait` step prints.
const WAIT_MATCH_LINES: usize = 3;

/// What one step did.
struct StepOutcome {
    ok: bool,
    message: String,
    /// Object refs of the elements the step acted on.
    targets: Vec<String>,
}

fn selector_of(target: &StepTarget) -> ElementSelector<'_> {
    ElementSelector {
        role: target.role.as_deref(),
        name: target.name.as_deref(),
        text: target.text.as_deref(),
        states: &[],
    }
}

fn query_matches(query: &ElementQuery, nodes: &[AccessibilityNode]) -> Vec<String> {
    nodes
        .iter()
        .filter(|node| query.matches(node))
        .map(crate::outline::render_line)
        .collect()
}

impl ComputerUseLinux {
    /// Runs `act`: validates every step (`plan_steps`), then runs them in
    /// order under the rules in the module doc of `act.rs`, and returns the
    /// step lines and one change report against the page as the agent last
    /// saw it.
    pub(super) async fn act(&self, params: ActParams) -> CallToolResult {
        let plans = match plan_steps(&params.steps) {
            Ok(plans) => plans,
            Err(message) => return error_result(format!("error: {message}; no step ran.")),
        };
        let start = self.cached_nodes();
        let total = plans.len();
        let mut lines = Vec::new();
        let mut busy_notes = String::new();
        let mut targets: Vec<String> = Vec::new();
        let mut last_settled: Option<Rescan> = None;
        let mut failed = false;
        let mut stop_note = None;
        for (at, plan) in plans.iter().enumerate() {
            let number = at + 1;
            let before = self.cached_nodes();
            let outcome = self.run_step(plan).await;
            for target in outcome.targets {
                targets.retain(|known| known != &target);
                targets.push(target);
            }
            lines.push(format!(
                "step {number} {}: {}",
                if outcome.ok { "ok" } else { "error" },
                outcome.message
            ));
            // Let the page settle so the next step finds what this one
            // revealed; a wait step has just read the page itself, and a
            // field followed by another field already in view needs no
            // settle between them (each is about 0.2 s of reads).
            let field_run = outcome.ok && self.next_field_ready(plan, plans.get(at + 1));
            if !field_run && !matches!(plan, StepPlan::Wait { .. }) {
                match self.settle(&before).await {
                    Ok(settled) => {
                        if let Some(note) = settled.busy_note {
                            busy_notes.push_str(&note);
                        }
                        let rescan = settled.rescan;
                        self.commit_observation(
                            &rescan.nodes,
                            rescan.root_pid,
                            Some(rescan.scope.clone()),
                        )
                        .await;
                        last_settled = Some(rescan);
                    }
                    Err(error) => lines.push(format!("(the page could not be read: {error})")),
                }
            } else {
                last_settled = self.cached_rescan().or(last_settled);
            }
            let now = self.cached_nodes();
            if !outcome.ok {
                failed = true;
                if let Some(dialog) = crate::page_view::blocking_dialog(&before, &now) {
                    lines.push(dialog);
                }
                if number < total {
                    stop_note = Some(format!(
                        "stopped at step {number} of {total}; steps {}-{total} did not run.",
                        number + 1
                    ));
                }
                break;
            }
            if let Some(next) = plans.get(at + 1) {
                if let Some(note) = self.unexpected_dialog(&before, &now, next) {
                    stop_note = Some(format!(
                        "stopped after step {number} of {total}: {note}; steps {}-{total} did not run.",
                        number + 1
                    ));
                    break;
                }
            }
        }
        let mut text = if failed {
            String::from("error: a step failed\n")
        } else if stop_note.is_some() {
            String::from("ok: stopped early\n")
        } else {
            format!("ok: {total}/{total} steps done\n")
        };
        for line in &lines {
            text.push_str(line);
            text.push('\n');
        }
        if let Some(note) = &stop_note {
            text.push_str(note);
            text.push('\n');
        }
        // A step that failed before any settle (its element was never
        // found) still leaves the read it searched in the cache.
        let last_settled = last_settled.or_else(|| self.cached_rescan());
        if let Some(settled) = &last_settled {
            // Elements a later step re-rendered or removed are in the change
            // report already; a `target:` line saying so is noise.
            let mut shown: Vec<String> = targets
                .iter()
                .filter(|object_ref| {
                    settled
                        .nodes
                        .iter()
                        .any(|node| &node.object_ref == *object_ref)
                })
                .cloned()
                .collect();
            shown.drain(..shown.len().saturating_sub(MAX_TARGET_LINES));
            let options = settled.scope.options;
            for notice in crate::page_view::new_notices(
                &crate::page_view::page_view(&start, options),
                &crate::page_view::page_view(&settled.nodes, options),
            ) {
                text.push_str(&format!("notice: {notice}\n"));
            }
            // With nothing observed before the call, every element would be
            // "new": the page itself says more.
            let observed_before = !start.is_empty();
            if observed_before {
                let report = self
                    .change_report_since(&start, settled, Some(busy_notes), &shown)
                    .await;
                text.push_str(&report);
            }
            for object_ref in &shown {
                let label = self.label_for_ref(object_ref);
                let (line, _) = self.target_line(object_ref, &label).await;
                text.push_str(&format!("target: {line}\n"));
            }
            // After a failure the agent needs the page to plan the rest:
            // print it here instead of costing it a get_app_state turn.
            if failed || stop_note.is_some() || !observed_before {
                text.push_str("page now:\n");
                text.push_str(&crate::page_view::page_view(&settled.nodes, options).outline());
            }
        }
        let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
        if failed {
            result.is_error = Some(true);
        }
        result
    }

    fn cached_nodes(&self) -> Vec<AccessibilityNode> {
        self.last_snapshot
            .lock()
            .map(|cached| cached.nodes.clone())
            .unwrap_or_default()
    }

    /// The cached observation as a `Rescan`, when it has a scope.
    fn cached_rescan(&self) -> Option<Rescan> {
        let cached = self.last_snapshot.lock().ok()?;
        Some(Rescan {
            nodes: cached.nodes.clone(),
            root_pid: cached.pid,
            scope: cached.scope.clone()?,
            read: 0,
        })
    }

    /// Reads the observed scope again and caches it.
    async fn refresh_cache(&self) -> Option<Vec<AccessibilityNode>> {
        let base = self.cached_nodes();
        let rescan = self.read_page(&base, None).await.ok()?;
        let nodes = rescan.nodes.clone();
        self.commit_observation(&rescan.nodes, rescan.root_pid, Some(rescan.scope))
            .await;
        Some(nodes)
    }

    /// Whether `plan` sets or types into a field and `next` does the same
    /// to a field the cached snapshot already resolves.
    fn next_field_ready(&self, plan: &StepPlan, next: Option<&StepPlan>) -> bool {
        if !matches!(plan, StepPlan::Set { .. } | StepPlan::Type { .. }) {
            return false;
        }
        let target = match next {
            Some(StepPlan::Set { target, .. }) => target,
            Some(StepPlan::Type {
                target: Some(target),
                ..
            }) => target,
            _ => return false,
        };
        self.resolve_cached_node(
            target.element_index,
            &selector_of(target),
            ElementResolvePurpose::SetValue,
        )
        .is_ok()
    }

    /// A dialog that opened during the last step (absent from `before`)
    /// while the next step does not act inside it: the note naming it.
    fn unexpected_dialog(
        &self,
        before: &[AccessibilityNode],
        now: &[AccessibilityNode],
        next: &StepPlan,
    ) -> Option<String> {
        let (dialog, note) = crate::page_view::blocking_dialog_with_id(before, now)?;
        if before.iter().any(|node| node.index == dialog) {
            return None;
        }
        let (target, purpose) = match next {
            StepPlan::Click { target, .. } => (Some(target), ElementResolvePurpose::Click),
            StepPlan::Set { target, .. } => (Some(target), ElementResolvePurpose::SetValue),
            StepPlan::Type { target, .. } => (target.as_ref(), ElementResolvePurpose::SetValue),
            StepPlan::Scroll {
                target: Some(id), ..
            } => {
                return (!crate::page_view::is_inside(now, *id, dialog)).then_some(note);
            }
            // Waiting for something is how an agent reacts to a dialog it
            // expects; a key press or scroll with no target is not.
            StepPlan::Wait { .. } => return None,
            _ => (None, ElementResolvePurpose::Click),
        };
        let inside = target.is_some_and(|target| match target.element_index {
            Some(id) => crate::page_view::is_inside(now, id, dialog),
            None => resolve_semantic_node(now, &selector_of(target), purpose)
                .is_ok_and(|node| crate::page_view::is_inside(now, node.index, dialog)),
        });
        (!inside).then_some(note)
    }

    /// Waits up to `STEP_FIND_LIMIT` for an element named by role/name/text
    /// to be in the cached snapshot, reading the page again meanwhile. Ids
    /// are left to the action itself. An ambiguous selector fails at once.
    async fn find_step_target(
        &self,
        target: &StepTarget,
        purpose: ElementResolvePurpose,
    ) -> Result<(), String> {
        if target.element_index.is_some() {
            return Ok(());
        }
        let selector = selector_of(target);
        let started = Instant::now();
        loop {
            let error = match self.resolve_cached_node(None, &selector, purpose) {
                Ok(_) => return Ok(()),
                Err(error) => error,
            };
            if self.cache_matches(&selector) || started.elapsed() >= STEP_FIND_LIMIT {
                return Err(error);
            }
            sleep(STEP_POLL).await;
            self.refresh_cache().await;
        }
    }

    async fn run_step(&self, plan: &StepPlan) -> StepOutcome {
        let described = describe_step(plan);
        let fail = |message: String| StepOutcome {
            ok: false,
            message: format!("{described}: {message}"),
            targets: Vec::new(),
        };
        let (target, purpose) = match plan {
            StepPlan::Click { target, .. } => (Some(target), ElementResolvePurpose::Click),
            StepPlan::Set { target, .. } => (Some(target), ElementResolvePurpose::SetValue),
            StepPlan::Type { target, .. } => (target.as_ref(), ElementResolvePurpose::SetValue),
            _ => (None, ElementResolvePurpose::Click),
        };
        let mut echo = Vec::new();
        if let Some(target) = target {
            if let Err(message) = self.find_step_target(target, purpose).await {
                // The act result shows the page after a failure.
                let message = message.replace(
                    " Call get_app_state first or pass element_index.",
                    " Pick an id from the page below.",
                );
                return fail(message);
            }
            echo = self
                .echo_target(target.element_index, None, &selector_of(target), purpose)
                .targets;
        }
        let (ok, message) = match plan {
            StepPlan::Click {
                target,
                click_count,
            } => {
                let Json(output) = self
                    .click(Parameters(ClickParams {
                        observe: Some(false),
                        element_index: target.element_index,
                        role: target.role.clone(),
                        name: target.name.clone(),
                        text: target.text.clone(),
                        click_count: (*click_count > 1).then_some(*click_count),
                        ..Default::default()
                    }))
                    .await;
                (output.ok, output.message)
            }
            StepPlan::Set { target, value } => {
                let Json(output) = self
                    .set_value(Parameters(SetValueParams {
                        observe: Some(false),
                        element_index: target.element_index,
                        role: target.role.clone(),
                        name: target.name.clone(),
                        text: target.text.clone(),
                        value: value.clone(),
                        ..Default::default()
                    }))
                    .await;
                (output.ok, output.message)
            }
            StepPlan::Type { target, text } => {
                let target = target.clone().unwrap_or_default();
                let (Json(output), unverified) = self
                    .send_text(TypeTextParams {
                        observe: Some(false),
                        text: text.clone(),
                        element_index: target.element_index,
                        role: target.role,
                        name: target.name.or(target.text),
                        ..Default::default()
                    })
                    .await;
                let mut message = output.message;
                if let Some(unverified) = unverified {
                    message.push_str(&unverified.note);
                }
                (output.ok, message)
            }
            StepPlan::Key { key } => {
                let Json(output) = self
                    .press_key(Parameters(PressKeyParams {
                        observe: Some(false),
                        key: key.clone(),
                        window_id: None,
                        pid: None,
                        tty: None,
                        terminal_pid: None,
                        terminal_command: None,
                        terminal_cwd: None,
                        app_id: None,
                        wm_class: None,
                        title: None,
                    }))
                    .await;
                (output.ok, output.message)
            }
            StepPlan::Scroll {
                target,
                until,
                direction,
            } => {
                let Json(output) = self
                    .scroll(Parameters(ScrollParams {
                        observe: Some(false),
                        element_index: *target,
                        x: None,
                        y: None,
                        direction: direction.clone(),
                        pages: None,
                        until: until.clone(),
                        window_id: None,
                        pid: None,
                        app_id: None,
                        wm_class: None,
                        window_title: None,
                        relative: None,
                    }))
                    .await;
                (output.ok, output.message)
            }
            StepPlan::Wait {
                query,
                condition,
                timeout,
            } => self.wait_step(query, *condition, *timeout).await,
        };
        StepOutcome {
            ok,
            message: if ok {
                message
            } else {
                format!("{described}: {message}")
            },
            targets: echo,
        }
    }

    /// A `wait` step: reads the observed scope until the condition holds or
    /// `timeout` passes, caching each read.
    async fn wait_step(
        &self,
        query: &ElementQuery,
        condition: WaitCondition,
        timeout: Duration,
    ) -> (bool, String) {
        let started = Instant::now();
        let first = crate::outline::element_contents(&self.cached_nodes());
        loop {
            if let Some(nodes) = self.refresh_cache().await {
                let matches = query_matches(query, &nodes);
                let done = match condition {
                    WaitCondition::Appears => !matches.is_empty(),
                    WaitCondition::Disappears => matches.is_empty(),
                    WaitCondition::Changed => crate::outline::element_contents(&nodes) != first,
                };
                if done {
                    let elapsed = started.elapsed().as_millis();
                    let found = match condition {
                        WaitCondition::Appears => format!(
                            ": {}",
                            matches
                                .iter()
                                .take(WAIT_MATCH_LINES)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join("; ")
                        ),
                        _ => String::new(),
                    };
                    return (true, format!("condition met after {elapsed} ms{found}"));
                }
            }
            if started.elapsed() >= timeout {
                return (
                    false,
                    format!("timed out after {} ms", started.elapsed().as_millis()),
                );
            }
            sleep(STEP_POLL).await;
        }
    }
}

fn error_result(text: String) -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
    result.is_error = Some(true);
    result
}
