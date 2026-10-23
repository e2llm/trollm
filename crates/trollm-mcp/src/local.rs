//! Deterministic, bounded tools supplied by the runtime without a transport.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::ToolDescriptor;

pub const SERVER: &str = "local";
pub const TOOL: &str = "calculate";

const SCALE: i128 = 1_000_000;
const MAX_SCALED: i128 = 999_999_999_999_999_999;
const INVALID_ARGUMENTS: &str = "local.calculate: invalid-arguments";
const DIVISION_BY_ZERO: &str = "local.calculate: division-by-zero";
const PRECISION_EXCEEDED: &str = "local.calculate: precision-exceeded";
const OUT_OF_RANGE: &str = "local.calculate: out-of-range";

pub fn descriptor() -> ToolDescriptor {
    ToolDescriptor {
        server: SERVER.into(),
        name: TOOL.into(),
        input_schema: serde_json::from_str(include_str!(
            "../../../schemas/local-calculate.v1.input.schema.json"
        ))
        .expect("the bundled local.calculate input schema must be valid JSON"),
    }
}

/// Calculate using exact signed decimals with at most six fractional digits.
/// Arguments are validated here even when no gateway schema check precedes us.
pub fn calculate(args: &Value) -> Result<Value> {
    let object = args.as_object().ok_or_else(|| anyhow!(INVALID_ARGUMENTS))?;
    if object.len() != 3 {
        bail!(INVALID_ARGUMENTS);
    }
    let string_field = |name| {
        object
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!(INVALID_ARGUMENTS))
    };
    let operation = string_field("operation")?;
    let left = string_field("left")?;
    let right = string_field("right")?;
    let a = parse_decimal(left)?;
    let b = parse_decimal(right)?;
    let result = match operation {
        "compare" => match a.cmp(&b) {
            std::cmp::Ordering::Less => "less",
            std::cmp::Ordering::Equal => "equal",
            std::cmp::Ordering::Greater => "greater",
        }
        .to_owned(),
        "add" | "subtract" | "multiply" | "divide" => {
            // Inputs are below 10^18 in scaled units. Thus even the product
            // (below 10^36) and division numerator (below 10^24) fit i128.
            let scaled = match operation {
                "add" => a + b,
                "subtract" => a - b,
                "multiply" => exact_quotient(a * b, SCALE)?,
                "divide" => {
                    if b == 0 {
                        bail!(DIVISION_BY_ZERO);
                    }
                    exact_quotient(a * SCALE, b)?
                }
                _ => unreachable!(),
            };
            if !(-MAX_SCALED..=MAX_SCALED).contains(&scaled) {
                bail!(OUT_OF_RANGE);
            }
            format_decimal(scaled)
        }
        _ => bail!(INVALID_ARGUMENTS),
    };
    Ok(json!({
        "format": "trollm.local.calculate.v1",
        "operation": operation,
        "left": left,
        "right": right,
        "result": result,
    }))
}

fn exact_quotient(numerator: i128, denominator: i128) -> Result<i128> {
    if numerator % denominator != 0 {
        bail!(PRECISION_EXCEEDED);
    }
    Ok(numerator / denominator)
}

fn parse_decimal(input: &str) -> Result<i128> {
    // A sign, twelve integer digits, a point and six fractional digits is
    // the largest accepted representation. Check before examining any digits.
    if input.is_empty() || input.len() > 20 {
        bail!(INVALID_ARGUMENTS);
    }
    let (negative, magnitude) = match input.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, input),
    };
    let (integer, fraction) = match magnitude.split_once('.') {
        Some((integer, fraction)) => {
            if fraction.is_empty() || fraction.len() > 6 {
                bail!(INVALID_ARGUMENTS);
            }
            (integer, fraction)
        }
        None => (magnitude, ""),
    };
    if integer.is_empty()
        || integer.len() > 12
        || (integer.len() > 1 && integer.starts_with('0'))
        || !integer.bytes().all(|digit| digit.is_ascii_digit())
        || !fraction.bytes().all(|digit| digit.is_ascii_digit())
    {
        bail!(INVALID_ARGUMENTS);
    }
    let digits = |part: &str| {
        part.bytes()
            .fold(0_i128, |value, digit| value * 10 + i128::from(digit - b'0'))
    };
    let scaled =
        digits(integer) * SCALE + digits(fraction) * 10_i128.pow(6 - fraction.len() as u32);
    Ok(if negative { -scaled } else { scaled })
}

fn format_decimal(scaled: i128) -> String {
    let magnitude = scaled.abs();
    let sign = if scaled < 0 { "-" } else { "" };
    let integer = magnitude / SCALE;
    let fraction = magnitude % SCALE;
    if fraction == 0 {
        format!("{sign}{integer}")
    } else {
        let fraction = format!("{fraction:06}");
        format!("{sign}{integer}.{}", fraction.trim_end_matches('0'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(operation: &str, left: &str, right: &str) -> Value {
        json!({"operation": operation, "left": left, "right": right})
    }

    fn assert_result(operation: &str, left: &str, right: &str, result: &str) {
        assert_eq!(
            calculate(&args(operation, left, right)).unwrap(),
            json!({
                "format": "trollm.local.calculate.v1",
                "operation": operation,
                "left": left,
                "right": right,
                "result": result,
            })
        );
    }

    #[test]
    fn exact_decimal_arithmetic_and_canonical_output() {
        assert_result("add", "0.1", "0.2", "0.3");
        assert_result("subtract", "-2.50", "-0.5", "-2");
        assert_result("multiply", "-1.250", "0.2", "-0.25");
        assert_result("divide", "-1", "8", "-0.125");
        assert_result("divide", "1", "-8", "-0.125");
        assert_result("multiply", "-0.000000", "-999999999999.999999", "0");
        assert_result("add", "-0", "0.000000", "0");
        assert_result("subtract", "0", "0.000001", "-0.000001");
        assert_result("divide", "0.000001", "0.000001", "1");
    }

    #[test]
    fn compare_uses_numeric_values_and_preserves_input_spelling() {
        assert_result("compare", "-0.000000", "0", "equal");
        assert_result("compare", "2", "10", "less");
        assert_result("compare", "-0.000001", "-1", "greater");
    }

    #[test]
    fn arithmetic_boundaries() {
        assert_result(
            "add",
            "999999999999.999998",
            "0.000001",
            "999999999999.999999",
        );
        assert_result(
            "multiply",
            "-999999999999.999999",
            "1",
            "-999999999999.999999",
        );
        assert_result("divide", "999999999999.999999", "1", "999999999999.999999");
        for value in [
            args("add", "999999999999.999999", "0.000001"),
            args("subtract", "-999999999999.999999", "0.000001"),
            args("multiply", "999999999999", "2"),
            args("multiply", "999999999999", "999999999999"),
            args("divide", "999999999999", "0.000001"),
        ] {
            assert_eq!(calculate(&value).unwrap_err().to_string(), OUT_OF_RANGE);
        }
    }

    #[test]
    fn rejects_inexact_results_and_zero_division() {
        for value in [
            args("divide", "1", "3"),
            args("multiply", "0.000001", "0.1"),
        ] {
            assert_eq!(
                calculate(&value).unwrap_err().to_string(),
                PRECISION_EXCEEDED
            );
        }
        for zero in ["0", "-0", "0.000000", "-0.000000"] {
            assert_eq!(
                calculate(&args("divide", "1", zero))
                    .unwrap_err()
                    .to_string(),
                DIVISION_BY_ZERO
            );
        }
    }

    #[test]
    fn rejects_invalid_decimal_spellings_without_echoing_them() {
        for input in [
            "",
            "-",
            "+1",
            "01",
            "-01",
            ".1",
            "1.",
            "1.2.3",
            " 1",
            "1\n",
            "1e2",
            "1,000",
            "NaN",
            "١",
            "1000000000000",
            "0.0000001",
            "--1",
        ] {
            for value in [args("add", input, "0"), args("add", "0", input)] {
                assert_eq!(
                    calculate(&value).unwrap_err().to_string(),
                    INVALID_ARGUMENTS
                );
            }
        }
        let oversized = "9".repeat(10_000);
        assert_eq!(
            calculate(&args("add", &oversized, "0"))
                .unwrap_err()
                .to_string(),
            INVALID_ARGUMENTS
        );
    }

    #[test]
    fn rejects_bad_objects_types_and_operations() {
        for value in [
            Value::Null,
            json!([]),
            json!({}),
            json!({"operation": "add", "left": "1"}),
            json!({"operation": "add", "left": "1", "right": "2", "extra": null}),
            json!({"operation": "add", "left": 1, "right": "2"}),
            json!({"operation": "add", "left": "1", "right": false}),
            json!({"operation": 1, "left": "1", "right": "2"}),
            args("pow", "1", "2"),
        ] {
            assert_eq!(
                calculate(&value).unwrap_err().to_string(),
                INVALID_ARGUMENTS
            );
        }
    }

    #[test]
    fn published_contract_cases() {
        let input_schema = crate::schema::ToolSchema::compile(&descriptor().input_schema).unwrap();
        let output_schema = crate::schema::ToolSchema::compile_result(
            &serde_json::from_str(include_str!(
                "../../../schemas/local-calculate.v1.output.schema.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../../schemas/local-calculate.v1.cases.json"
        ))
        .unwrap();
        let cases = fixtures["cases"].as_array().unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            assert_eq!(
                input_schema.validate(&case["args"]).is_ok(),
                case["error"] != INVALID_ARGUMENTS,
                "argument schema disagrees for case {}",
                case["name"]
            );
            let actual = calculate(&case["args"]);
            if let Some(expected) = case.get("result") {
                output_schema.validate_result(expected, 1024).unwrap();
                assert_eq!(actual.unwrap(), *expected, "case {}", case["name"]);
            } else {
                assert_eq!(
                    actual.unwrap_err().to_string(),
                    case["error"].as_str().unwrap(),
                    "case {}",
                    case["name"]
                );
            }
        }
    }
}
