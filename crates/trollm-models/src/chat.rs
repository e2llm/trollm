//! Text-only rendering of the chat template embedded in a GGUF model.

use anyhow::{bail, Context, Result};
use minijinja::{context, AutoEscape, Environment, Error, ErrorKind, UndefinedBehavior};

/// Render the ledger's complete prompt as one user message. Thinking is
/// disabled through the template's own configuration; tool fences stay in the
/// message rather than being translated into model-native tool declarations.
pub fn render_chat(
    template: &str,
    prompt: &str,
    bos_token: &str,
    eos_token: &str,
) -> Result<String> {
    if template.trim().is_empty() {
        bail!("GGUF chat template is missing or empty");
    }
    let mut environment = Environment::new();
    environment.set_auto_escape_callback(|_| AutoEscape::None);
    environment.set_undefined_behavior(UndefinedBehavior::SemiStrict);
    environment.set_fuel(Some(1_000_000));
    environment.set_unknown_method_callback(minijinja_contrib::pycompat::unknown_method_callback);
    environment.add_function(
        "raise_exception",
        |message: String| -> Result<String, Error> {
            Err(Error::new(ErrorKind::InvalidOperation, message))
        },
    );
    // No template loader or filesystem/network callbacks are registered.
    let compiled = environment
        .template_from_str(template)
        .context("invalid GGUF Jinja chat template")?;
    compiled
        .render(context! {
            messages => [serde_json::json!({"role": "user", "content": prompt})],
            add_generation_prompt => true,
            enable_thinking => false,
            bos_token => bos_token,
            eos_token => eos_token,
            tools => Vec::<serde_json::Value>::new(),
        })
        .context("failed to render GGUF Jinja chat template")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_non_thinking_reply_and_model_end_tags() {
        let template = "{{ bos_token }}{% for message in messages %}<user>{{ message.content }}</user>{{ eos_token }}{% endfor %}{% if add_generation_prompt %}<assistant>{% if enable_thinking %}<think>{% else %}<think></think>{% endif %}{% endif %}";
        assert_eq!(
            render_chat(template, "hello", "<bos>", "<eos>").unwrap(),
            "<bos><user>hello</user><eos><assistant><think></think>"
        );
    }

    #[test]
    fn prompt_is_data_and_python_string_methods_work() {
        let template = "{{ messages[0].get('content').strip().lstrip('x').rstrip('x') }}";
        assert_eq!(
            render_chat(template, " x{{ 7 * 7 }}x ", "", "").unwrap(),
            "{{ 7 * 7 }}"
        );
    }

    #[test]
    fn invalid_or_unconfigured_templates_fail_explicitly() {
        for template in [
            "",
            "{% if %}",
            "{{ required_configuration }}",
            "{{ raise_exception('unsupported configuration') }}",
        ] {
            assert!(
                render_chat(template, "hello", "", "").is_err(),
                "accepted {template}"
            );
        }
    }

    #[test]
    fn runaway_template_runs_out_of_fuel() {
        let error = render_chat(
            "{% for x in range(2000) %}{% for y in range(2000) %}{{ x }}{% endfor %}{% endfor %}",
            "hello",
            "",
            "",
        )
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<Error>().unwrap().kind(),
            ErrorKind::OutOfFuel
        );
    }
}
