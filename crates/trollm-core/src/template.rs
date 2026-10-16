//! Hand-rolled minimal `{{ var }}` template substitution (§5 task loop
//! semantics: "render prompt"). Deliberately tiny: `{{ dotted.path }}` against
//! the bound frame vars, with `user_input` injected. Strings render verbatim;
//! other values render as JSON (the ledger handles all structured data —
//! models only ever see it serialized into their prompt).

use serde_json::Value;

#[derive(Debug, thiserror::Error)]
#[error("template variable '{0}' is not bound")]
pub struct TemplateError(pub String);

/// Render `template` against frame `vars`; `user_input` is bound verbatim and
/// never stored in frame vars (§3: promptTemplate "may reference {{ user_input }}").
pub fn render(template: &str, vars: &Value, user_input: &str) -> Result<String, TemplateError> {
    let mut merged = match vars {
        Value::Object(m) => m.clone(),
        _ => serde_json::Map::new(),
    };
    merged.insert(
        "user_input".to_string(),
        Value::String(user_input.to_string()),
    );
    let vars = Value::Object(merged);

    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .ok_or_else(|| TemplateError("<unterminated {{".to_string()))?;
        let name = after[..end].trim();
        let value = lookup(name, &vars).ok_or_else(|| TemplateError(name.to_string()))?;
        match value {
            Value::String(s) => out.push_str(s),
            other => out.push_str(&serde_json::to_string(other).unwrap_or_default()),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Resolve a dotted path (`item.id`, `each.outputs.results`) against the vars object.
fn lookup<'a>(path: &str, vars: &'a Value) -> Option<&'a Value> {
    let mut cur = vars;
    for part in path.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dotted_paths_and_user_input() {
        let vars =
            json!({"item": {"id": 7, "brief": "x"}, "each": {"outputs": {"results": [1, 2]}}});
        let out = render(
            "id={{ item.id }} ui={{ user_input }} res={{ each.outputs.results }}",
            &vars,
            "close old tickets",
        )
        .unwrap();
        assert_eq!(out, "id=7 ui=close old tickets res=[1,2]");
    }

    #[test]
    fn missing_var_is_descriptive_error() {
        let err = render("{{ nope }}", &json!({}), "").unwrap_err();
        assert!(err.to_string().contains("nope"));
    }
}
