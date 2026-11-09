use super::*;

pub(in crate::rust) fn collect_numeric_local_slots(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
    slots: &mut HashSet<usize>,
    active: &mut HashSet<String>,
) -> bool {
    match input.and_then(parse_input) {
        Some(InputValue::Number(_) | InputValue::String(_)) => true,
        Some(InputValue::VariableReference(reference)) => {
            super::super::project::variable_for_target(target, reference.id, variables)
                .filter(|variable| variable.numeric)
                .is_some_and(|variable| {
                    slots.insert(variable.slot);
                    true
                })
        }
        Some(InputValue::BlockReference(id)) => {
            if !active.insert(id.to_owned()) {
                return false;
            }
            let result = target.blocks.get(id).is_some_and(|block| {
                if !matches!(
                    block.opcode.as_str(),
                    "operator_add"
                        | "operator_subtract"
                        | "operator_multiply"
                        | "operator_divide"
                        | "operator_mod"
                        | "operator_equals"
                        | "operator_gt"
                        | "operator_lt"
                        | "operator_and"
                        | "operator_or"
                        | "operator_not"
                        | "operator_round"
                        | "operator_mathop"
                        | "data_variable"
                        | "math_number"
                        | "math_integer"
                        | "math_whole_number"
                        | "math_positive_number"
                        | "math_angle"
                        | "text"
                ) {
                    return false;
                }
                if block.opcode == "data_variable" {
                    return variable_field(block, "VARIABLE", variables)
                        .filter(|variable| variable.numeric)
                        .is_some_and(|variable| {
                            slots.insert(variable.slot);
                            true
                        });
                }
                block.inputs.values().all(|input| {
                    collect_numeric_local_slots(target, Some(input), variables, slots, active)
                })
            });
            active.remove(id);
            result
        }
        _ => false,
    }
}

// lower an expression using f64 locals for variables proven numeric project-wide.
pub(in crate::rust) fn local_expression(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
    locals: &HashMap<usize, String>,
    fast_math: bool,
    active: &mut HashSet<String>,
) -> Option<LocalExpression> {
    match input.and_then(parse_input)? {
        InputValue::Number(value) => Some(LocalExpression {
            code: number_literal(value),
            kind: LocalValueKind::Number,
        }),
        InputValue::String(value) => Some(LocalExpression {
            code: text_literal(value),
            kind: LocalValueKind::Text,
        }),
        InputValue::VariableReference(reference) => {
            let variable =
                super::super::project::variable_for_target(target, reference.id, variables)?;
            Some(LocalExpression {
                code: locals.get(&variable.slot)?.clone(),
                kind: LocalValueKind::Number,
            })
        }
        InputValue::BlockReference(id) => {
            if !active.insert(id.to_owned()) {
                return None;
            }
            let result = target.blocks.get(id).and_then(|block| {
                local_block_expression(target, block, variables, locals, fast_math, active)
            });
            active.remove(id);
            result
        }
        _ => None,
    }
}

pub(in crate::rust) fn expression_argument(expression: &str) -> &str {
    let bytes = expression.as_bytes();
    if bytes.first() != Some(&b'(') || bytes.last() != Some(&b')') {
        return expression;
    }

    let mut depth = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }

        match byte {
            b'"' => in_string = true,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 && index + 1 != bytes.len() {
                    return expression;
                }
            }
            _ => {}
        }
    }

    if depth == 0 {
        &expression[1..expression.len() - 1]
    } else {
        expression
    }
}

pub(super) fn local_block_expression(
    target: &ScratchTarget,
    block: &ScratchBlock,
    variables: &HashMap<String, VariableInfo>,
    locals: &HashMap<usize, String>,
    fast_math: bool,
    active: &mut HashSet<String>,
) -> Option<LocalExpression> {
    let input = |name: &str, active: &mut HashSet<String>| {
        local_expression(
            target,
            block.inputs.get(name),
            variables,
            locals,
            fast_math,
            active,
        )
    };
    let numeric_binary =
        |function: &str, operator: &str, left: &str, right: &str, active: &mut HashSet<String>| {
            let left = input(left, active)?;
            let right = input(right, active)?;
            let native =
                left.kind == LocalValueKind::Number && right.kind == LocalValueKind::Number;
            let code = if native {
                let left = if fast_math {
                    left.code
                } else {
                    format!(
                        "scratch_number_operand({})",
                        expression_argument(&left.code)
                    )
                };
                let right = if fast_math {
                    right.code
                } else {
                    format!(
                        "scratch_number_operand({})",
                        expression_argument(&right.code)
                    )
                };
                format!("({left} {operator} {right})")
            } else {
                format!(
                    "{function}({}, {})",
                    expression_argument(&left.code),
                    expression_argument(&right.code)
                )
            };
            Some(LocalExpression {
                code,
                kind: LocalValueKind::Number,
            })
        };
    let comparison = |function: &str,
                      operator: &str,
                      left_name: &str,
                      right_name: &str,
                      active: &mut HashSet<String>| {
        let left = input(left_name, active)?;
        let right = input(right_name, active)?;
        let native = left.kind == LocalValueKind::Number
            && right.kind == LocalValueKind::Number
            && local_input_is_non_nan_number(
                target,
                block.inputs.get(left_name),
                variables,
                &mut HashSet::new(),
            )
            && local_input_is_non_nan_number(
                target,
                block.inputs.get(right_name),
                variables,
                &mut HashSet::new(),
            );
        Some(LocalExpression {
            code: if native {
                format!("{} {operator} {}", left.code, right.code)
            } else {
                format!("{function}({}, {})", left.code, right.code)
            },
            kind: LocalValueKind::Boolean,
        })
    };

    match block.opcode.as_str() {
        "operator_add" => numeric_binary("scratch_add", "+", "NUM1", "NUM2", active),
        "operator_subtract" => numeric_binary("scratch_subtract", "-", "NUM1", "NUM2", active),
        "operator_multiply" => numeric_binary("scratch_multiply", "*", "NUM1", "NUM2", active),
        "operator_divide" => numeric_binary("scratch_divide", "/", "NUM1", "NUM2", active),
        "operator_mod" => {
            let left = input("NUM1", active)?;
            let right = input("NUM2", active)?;
            Some(LocalExpression {
                code: format!("scratch_mod({}, {})", left.code, right.code),
                kind: LocalValueKind::Number,
            })
        }
        "operator_equals" if identical_literal_inputs(block) => Some(LocalExpression {
            code: "true".to_owned(),
            kind: LocalValueKind::Boolean,
        }),
        "operator_equals" => comparison("scratch_equals", "==", "OPERAND1", "OPERAND2", active),
        "operator_gt" => comparison("scratch_greater_than", ">", "OPERAND1", "OPERAND2", active),
        "operator_lt" => comparison("scratch_less_than", "<", "OPERAND1", "OPERAND2", active),
        "operator_and" | "operator_or" => {
            let left = local_to_bool(input("OPERAND1", active)?);
            let right = local_to_bool(input("OPERAND2", active)?);
            let operator = if block.opcode == "operator_and" {
                "&&"
            } else {
                "||"
            };
            Some(LocalExpression {
                code: format!("({left} {operator} {right})"),
                kind: LocalValueKind::Boolean,
            })
        }
        "operator_not" => Some(LocalExpression {
            code: format!("!({})", local_to_bool(input("OPERAND", active)?)),
            kind: LocalValueKind::Boolean,
        }),
        "operator_round" => Some(LocalExpression {
            code: format!("scratch_round({})", local_to_number(input("NUM", active)?)),
            kind: LocalValueKind::Number,
        }),
        "operator_mathop" => {
            let value = input("NUM", active)?;
            let numeric = if value.kind == LocalValueKind::Number {
                if fast_math {
                    value.code.clone()
                } else {
                    format!(
                        "scratch_number_operand({})",
                        expression_argument(&value.code)
                    )
                }
            } else {
                format!("scratch_to_number({})", expression_argument(&value.code))
            };
            let operator = field_value(block, "OPERATOR")?.as_str()?;
            let numeric_trig_input = value.kind == LocalValueKind::Number;
            let code = match operator {
                "sin" if !fast_math && numeric_trig_input => {
                    format!("scratch_sin_number({numeric})")
                }
                "sin" => format!(
                    "{}({})",
                    if fast_math { "fast_sin" } else { "scratch_sin" },
                    expression_argument(&value.code)
                ),
                "cos" if !fast_math && numeric_trig_input => {
                    format!("scratch_cos_number({numeric})")
                }
                "cos" => format!(
                    "{}({})",
                    if fast_math { "fast_cos" } else { "scratch_cos" },
                    expression_argument(&value.code)
                ),
                "tan" => format!(
                    "{}({})",
                    if fast_math { "fast_tan" } else { "scratch_tan" },
                    expression_argument(&value.code)
                ),
                "asin" if !fast_math && numeric_trig_input => {
                    format!("scratch_asin_degrees_number({numeric})")
                }
                "asin" => format!(
                    "{}({})",
                    if fast_math {
                        "fast_asin_degrees"
                    } else {
                        "scratch_asin_degrees"
                    },
                    expression_argument(&value.code)
                ),
                "acos" => format!(
                    "{}({})",
                    if fast_math {
                        "fast_acos_degrees"
                    } else {
                        "scratch_acos_degrees"
                    },
                    expression_argument(&value.code)
                ),
                "atan" => format!(
                    "{}({})",
                    if fast_math {
                        "fast_atan_degrees"
                    } else {
                        "scratch_atan_degrees"
                    },
                    expression_argument(&value.code)
                ),
                "abs" => format!("({numeric}).abs()"),
                "floor" => format!("({numeric}).floor()"),
                "ceiling" => format!("({numeric}).ceil()"),
                "sqrt" => format!("({numeric}).sqrt()"),
                "ln" => format!("({numeric}).ln()"),
                "log" => format!("({numeric}).log10()"),
                "e ^" => format!("({numeric}).exp()"),
                "10 ^" => format!("10.0_f64.powf({numeric})"),
                _ => return None,
            };
            Some(LocalExpression {
                code,
                kind: LocalValueKind::Number,
            })
        }
        "data_variable" => {
            let variable = variable_field(block, "VARIABLE", variables)?;
            Some(LocalExpression {
                code: locals.get(&variable.slot)?.clone(),
                kind: LocalValueKind::Number,
            })
        }
        "math_number"
        | "math_integer"
        | "math_whole_number"
        | "math_positive_number"
        | "math_angle" => Some(LocalExpression {
            code: number_literal(field_value(block, "NUM")?),
            kind: LocalValueKind::Number,
        }),
        "text" => Some(LocalExpression {
            code: text_literal(field_value(block, "TEXT")?),
            kind: LocalValueKind::Text,
        }),
        _ => None,
    }
}

pub(super) fn local_input_is_non_nan_number(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
    active: &mut HashSet<String>,
) -> bool {
    match input.and_then(parse_input) {
        Some(InputValue::Number(value)) => value
            .as_f64()
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
            .is_some_and(|value: f64| !value.is_nan()),
        Some(InputValue::VariableReference(reference)) => {
            super::super::project::variable_for_target(target, reference.id, variables)
                .is_some_and(|variable| variable.non_nan)
        }
        Some(InputValue::BlockReference(id)) => {
            if !active.insert(id.to_owned()) {
                return false;
            }
            let result = target.blocks.get(id).is_some_and(|block| {
                if block.opcode == "data_variable" {
                    return variable_field(block, "VARIABLE", variables)
                        .is_some_and(|variable| variable.non_nan);
                }
                matches!(
                    block.opcode.as_str(),
                    "math_number"
                        | "math_integer"
                        | "math_whole_number"
                        | "math_positive_number"
                        | "math_angle"
                ) && field_value(block, "NUM")
                    .and_then(|value| {
                        value
                            .as_f64()
                            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
                    })
                    .is_some_and(|value: f64| !value.is_nan())
            });
            active.remove(id);
            result
        }
        _ => false,
    }
}

pub(in crate::rust) fn local_to_number(value: LocalExpression) -> String {
    match value.kind {
        LocalValueKind::Number => value.code,
        LocalValueKind::Boolean => format!("if {} {{ 1.0_f64 }} else {{ 0.0_f64 }}", value.code),
        LocalValueKind::Text => format!("scratch_to_number({})", value.code),
    }
}

pub(in crate::rust) fn local_to_bool(value: LocalExpression) -> String {
    match value.kind {
        LocalValueKind::Boolean => value.code,
        LocalValueKind::Number | LocalValueKind::Text => format!("scratch_bool({})", value.code),
    }
}

// lower math reporters, keeping literal numbers direct and converting dynamic inputs at runtime.
