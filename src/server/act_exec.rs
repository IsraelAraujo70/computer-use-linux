//! Runs `act` steps against the live app: the executor half of `act`
//! (types and validation live in `act.rs`).

use super::act::{plan_steps, ActParams};

impl super::ComputerUseLinux {
    /// Runs `act`: validates every step (`plan_steps`), then runs them in
    /// order under the rules in the module doc, and returns the step lines
    /// and one change report against the page as the agent last saw it.
    pub(super) async fn act(&self, params: ActParams) -> rmcp::model::CallToolResult {
        let text = match plan_steps(&params.steps) {
            Ok(_) => "error: act is not implemented yet".to_string(),
            Err(message) => format!("error: {message}"),
        };
        let mut result =
            rmcp::model::CallToolResult::success(vec![rmcp::model::ContentBlock::text(text)]);
        result.is_error = Some(true);
        result
    }
}
