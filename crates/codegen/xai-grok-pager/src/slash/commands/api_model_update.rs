// FORK PATCH 14 (api model-list refresh): BYOK `[model.*]` entries are hand-written,
// so a newly released Kimi or Opus is unusable until config.toml is edited by hand and a
// retired model lingers in the picker. This command asks every configured provider for
// its own live model list and reconciles `config.toml` against it. Reverting it drops
// `/api-model-update` (and its `/model-update` alias) and hands BYOK catalog upkeep back
// to the user's text editor; no other fork patch depends on it.
//!
//! Pager-side wrapper over the shell builtin of the same name: it shadows the
//! ACP-advertised entry so the dropdown can suggest `apply`, and every form passes
//! through to the shell unchanged. The discovery, reconcile, and write-back all live in
//! `xai_grok_shell::api_model_update`, which is where the network and file I/O belong.
//!
//! No args is the dry run (the plan IS the review step); `apply` writes; a bare word
//! restricts the run to providers whose `base_url` host contains it.

use crate::slash::command::{
    AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand, slash_meta,
};

/// Subcommands the shell accepts, offered as the first argument.
const UPDATE_OPS: [(&str, &str); 1] = [("apply", "Write the plan to config.toml (backs up first)")];

/// `/api-model-update`: refresh the BYOK model catalog from each provider's list.
pub struct ApiModelUpdateCommand;

impl SlashCommand for ApiModelUpdateCommand {
    slash_meta! {
        name: "api-model-update",
        aliases: ["model-update"],
        // Mirrors the shell builtin this command shadows.
        description: "Refresh BYOK [model.*] entries from each provider's live model list",
        usage: "/api-model-update [apply] [provider]",
        takes_args: true,
        session_scoped: true,
        arg_placeholder: "[apply] [provider substring]",
    }

    fn suggest_args(&self, _ctx: &AppCtx, args_query: &str) -> Option<Vec<ArgItem>> {
        // Only the first token is a known op; a provider substring is free text.
        if args_query.contains(char::is_whitespace) {
            return None;
        }
        Some(
            UPDATE_OPS
                .iter()
                .map(|&(op, description)| ArgItem {
                    display: op.to_string(),
                    match_text: op.to_string(),
                    insert_text: op.to_string(),
                    description: description.to_string(),
                })
                .collect(),
        )
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            return CommandResult::PassThrough("/api-model-update".to_string());
        }
        CommandResult::PassThrough(format!("/api-model-update {trimmed}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slash::registry::CommandRegistry;

    fn exec_ctx(models: &crate::acp::model_state::ModelState) -> CommandExecCtx<'_> {
        crate::slash::commands::tests::make_ctx(models)
    }

    #[test]
    fn bare_invocation_passes_through_as_dry_run() {
        let models = crate::acp::model_state::ModelState::default();
        let mut ctx = exec_ctx(&models);
        match ApiModelUpdateCommand.run(&mut ctx, "") {
            CommandResult::PassThrough(text) => assert_eq!(text, "/api-model-update"),
            other => panic!("expected PassThrough, got {other:?}"),
        }
        match ApiModelUpdateCommand.run(&mut ctx, "   ") {
            CommandResult::PassThrough(text) => assert_eq!(text, "/api-model-update"),
            other => panic!("expected PassThrough, got {other:?}"),
        }
    }

    #[test]
    fn args_reach_the_shell_verbatim() {
        let models = crate::acp::model_state::ModelState::default();
        let mut ctx = exec_ctx(&models);
        match ApiModelUpdateCommand.run(&mut ctx, "apply moonshot") {
            CommandResult::PassThrough(text) => {
                assert_eq!(text, "/api-model-update apply moonshot");
            }
            other => panic!("expected PassThrough, got {other:?}"),
        }
    }

    #[test]
    fn first_token_suggests_apply_then_stops() {
        let models = crate::acp::model_state::ModelState::default();
        let ctx = AppCtx {
            models: &models,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: false,
            saved_workflows: &[],
            workflow_runs: &[],
            screen_mode: crate::app::ScreenMode::Fullscreen,
            current_title: None,
        };
        let items = ApiModelUpdateCommand
            .suggest_args(&ctx, "")
            .expect("first phase suggests ops");
        assert_eq!(
            items.iter().map(|i| i.display.as_str()).collect::<Vec<_>>(),
            ["apply"]
        );
        assert!(
            ApiModelUpdateCommand.suggest_args(&ctx, "apply ").is_none(),
            "a provider substring is free text, not a suggestion"
        );
    }

    #[test]
    fn registered_under_both_trigger_keys() {
        let reg = CommandRegistry::new(crate::slash::commands::builtin_commands());
        let canonical = reg.get("api-model-update").expect("/api-model-update");
        assert_eq!(canonical.name(), "api-model-update");
        assert_eq!(
            reg.get("model-update").map(|c| c.name()),
            Some("api-model-update")
        );
    }
}
