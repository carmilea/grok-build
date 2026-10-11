//! Plan → `config.toml` edit, through `toml_edit` so everything the plan does
//! not name keeps its bytes, comments, and key order.

use super::reconcile::{AddPlan, ProviderOutcome, ProviderPlan};

/// Fields a new entry copies from its newest sibling: how to reach the
/// provider and how to authenticate there. Anything else (sampling knobs,
/// labels, per-model prompt settings) is left to the catalog defaults.
const INHERITED_KEYS: [&str; 11] = [
    "api_backend",
    "base_url",
    "api_base_url",
    "auth_scheme",
    "api_key",
    "env_key",
    "auth_provider",
    "model_provider",
    "extra_headers",
    "env_http_headers",
    "mtls_cert_dir",
];

/// Apply every reconciled provider's adds and trims to `original`.
///
/// Returns the new document text; the caller backs up and publishes it.
/// Providers that failed or went unchecked contribute nothing, so a failed
/// discovery leaves the file byte-identical.
pub(crate) fn apply_plan(
    original: &str,
    plans: &[ProviderPlan],
    added_on: &str,
) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = original
        .parse()
        .map_err(|e| format!("config.toml is not valid TOML: {e}"))?;

    if !plans.iter().any(ProviderPlan::writes_anything) {
        return Ok(doc.to_string());
    }

    let models = doc
        .get_mut("model")
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| "config.toml has no [model.*] tables to update".to_string())?;

    // Build every new table against the pristine document first: a plan may trim
    // the very entry another add inherits from, and the donor must still be there.
    let mut additions: Vec<(String, toml_edit::Table)> = Vec::new();
    for plan in plans {
        let ProviderOutcome::Reconciled { adds, .. } = &plan.outcome else {
            continue;
        };
        for add in adds {
            additions.push((add.id.clone(), new_entry_table(models, add, added_on)?));
        }
    }
    for plan in plans {
        let ProviderOutcome::Reconciled { trims, .. } = &plan.outcome else {
            continue;
        };
        for key in trims {
            models.remove(key);
        }
    }
    for (id, table) in additions {
        models.insert(&id, toml_edit::Item::Table(table));
    }

    Ok(doc.to_string())
}

/// Build the `[model.<id>]` table for one add, copying the sibling's endpoint
/// and credential keys verbatim (inline tables and comments included).
fn new_entry_table(
    models: &toml_edit::Table,
    add: &AddPlan,
    added_on: &str,
) -> Result<toml_edit::Table, String> {
    let sibling = models
        .get(&add.sibling_key)
        .and_then(toml_edit::Item::as_table)
        .ok_or_else(|| {
            format!(
                "[model.{}] is gone from config.toml; nothing written for {}",
                add.sibling_key, add.id
            )
        })?;

    // The planner filters ids that already name a table; re-check here so a
    // future caller cannot silently overwrite a hand-written entry.
    if models.contains_key(&add.id) {
        return Err(format!(
            "[model.{}] already exists; nothing written",
            add.id
        ));
    }

    let mut table = toml_edit::Table::new();
    table.set_implicit(false);
    table.insert("model", toml_edit::value(add.id.as_str()));
    for key in INHERITED_KEYS {
        if let Some(item) = sibling.get(key) {
            table.insert(key, item.clone());
        }
    }
    match add.context_window {
        Some(window) => {
            table.insert(
                "context_window",
                toml_edit::value(i64::try_from(window).unwrap_or(i64::MAX)),
            );
        }
        None => {
            if let Some(item) = sibling.get("context_window") {
                table.insert("context_window", item.clone());
            }
        }
    }
    if add.reasoning_efforts.is_empty() {
        if let Some(item) = sibling.get("reasoning_efforts") {
            table.insert("reasoning_efforts", item.clone());
        }
    } else {
        let mut efforts = toml_edit::Array::new();
        for effort in &add.reasoning_efforts {
            efforts.push(effort.as_str());
        }
        table.insert("reasoning_efforts", toml_edit::value(efforts));
    }
    table
        .decor_mut()
        .set_prefix(format!("\n# added by /api-model-update on {added_on}\n"));
    Ok(table)
}
