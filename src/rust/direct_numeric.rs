//! Guarded numeric locals for bounded, non-suspending direct procedures.
//!
//! The dynamic procedure remains authoritative for noncanonical inputs. Writes
//! with no observers may be committed at the end; reads within the call are cached.

use super::{
    expressions::expression,
    literals::{field_value, line, number_literal},
    project::{ListInfo, VariableInfo},
    scripts::ProcedureSignature,
    statements::{procedure_for_call, variable_field, variable_write_mask},
};
use crate::parser::{InputValue, ScratchBlock, ScratchTarget, parse_input};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Number,
    Boolean,
}
struct NumericExpr {
    code: String,
    kind: Kind,
}

struct Compiler<'a> {
    target: &'a ScratchTarget,
    variables: &'a HashMap<String, VariableInfo>,
    argument_values: HashMap<String, String>,
    lists: &'a HashMap<String, ListInfo>,
    procedures: &'a [ProcedureSignature],
    arguments: HashMap<String, usize>,
    all: BTreeMap<usize, VariableInfo>,
    needed: BTreeMap<usize, VariableInfo>,
    assigned: BTreeMap<usize, VariableInfo>,
    number_assignments: HashSet<usize>,
    dynamic_assignments: HashSet<usize>,
    statements: usize,
}

impl<'a> Compiler<'a> {
    fn deferred_slots(&self) -> HashSet<usize> {
        self.number_assignments
            .difference(&self.dynamic_assignments)
            .copied()
            .collect()
    }

    fn emit_deferred_commits(&self, out: &mut String, indent: usize) {
        let deferred = self.deferred_slots();
        for variable in self
            .assigned
            .values()
            .filter(|variable| deferred.contains(&variable.slot))
        {
            let setter = if variable.is_stage {
                "set_shared_number"
            } else {
                "set_existing_local_number"
            };
            line(
                out,
                indent,
                &format!(
                    "if scratch_defer_writes && scratch_dirty_{} {{ state.{setter}({}, numeric_{}); }}",
                    variable.slot, variable.slot, variable.slot
                ),
            );
        }
    }

    fn read_variable(&mut self, variable: VariableInfo, written: &HashSet<usize>) -> NumericExpr {
        self.all.insert(variable.slot, variable.clone());
        if !written.contains(&variable.slot) {
            self.needed.insert(variable.slot, variable.clone());
        }
        NumericExpr {
            code: format!("numeric_{}", variable.slot),
            kind: Kind::Number,
        }
    }

    fn expr(
        &mut self,
        input: Option<&Value>,
        written: &HashSet<usize>,
        active: &mut HashSet<String>,
    ) -> Option<NumericExpr> {
        match input.and_then(parse_input)? {
            InputValue::Number(value) => Some(NumericExpr {
                code: number_literal(value),
                kind: Kind::Number,
            }),
            InputValue::String(value) => {
                let text = value.as_str()?;
                let n = text.parse::<i32>().ok()?;
                if n.unsigned_abs() > 1_000_000 || n.to_string() != text {
                    return None;
                }
                Some(NumericExpr {
                    code: format!("{n}.0_f64"),
                    kind: Kind::Number,
                })
            }
            InputValue::VariableReference(reference) => {
                let variable =
                    super::project::variable_for_target(self.target, reference.id, self.variables)?
                        .clone();
                Some(self.read_variable(variable, written))
            }
            InputValue::BlockReference(id) => {
                if !active.insert(id.to_owned()) {
                    return None;
                }
                let block = self.target.blocks.get(id)?;
                let result = self.block_expr(block, written, active);
                active.remove(id);
                result
            }
            _ => None,
        }
    }

    fn block_expr(
        &mut self,
        block: &ScratchBlock,
        written: &HashSet<usize>,
        active: &mut HashSet<String>,
    ) -> Option<NumericExpr> {
        let opcode = block.opcode.as_str();
        if opcode == "data_variable" {
            return Some(
                self.read_variable(variable_field(block, "VARIABLE", self.variables)?, written),
            );
        }
        if matches!(
            opcode,
            "argument_reporter_string_number" | "argument_reporter_boolean"
        ) {
            let name = field_value(block, "VALUE")?.as_str()?;
            return Some(NumericExpr {
                code: format!("numeric_argument_{}", self.arguments.get(name)?),
                kind: Kind::Number,
            });
        }
        if opcode == "operator_not" {
            let input = self.expr(block.inputs.get("OPERAND"), written, active)?;
            return Some(NumericExpr {
                code: format!("!({})", boolean(input)),
                kind: Kind::Boolean,
            });
        }
        if opcode == "operator_mathop" {
            let input = self.expr(block.inputs.get("NUM"), written, active)?;
            if input.kind != Kind::Number {
                return None;
            }
            let operator = field_value(block, "OPERATOR")?.as_str()?;
            let code = match operator {
                "sin" => format!("scratch_sin({})", input.code),
                "cos" => format!("scratch_cos({})", input.code),
                "tan" => format!("scratch_tan({})", input.code),
                "asin" => format!("scratch_asin_degrees({})", input.code),
                "acos" => format!("scratch_acos_degrees({})", input.code),
                "atan" => format!("scratch_atan_degrees({})", input.code),
                "abs" => format!("({}).abs()", input.code),
                "floor" => format!("({}).floor()", input.code),
                "ceiling" => format!("({}).ceil()", input.code),
                "sqrt" => format!("({}).sqrt()", input.code),
                "ln" => format!("({}).ln()", input.code),
                "log" => format!("({}).log10()", input.code),
                "e ^" => format!("({}).exp()", input.code),
                "10 ^" => format!("10.0_f64.powf({})", input.code),
                _ => return None,
            };
            return Some(NumericExpr {
                code,
                kind: Kind::Number,
            });
        }
        let (left, right) = if matches!(
            opcode,
            "operator_add" | "operator_subtract" | "operator_multiply" | "operator_divide"
        ) {
            ("NUM1", "NUM2")
        } else {
            ("OPERAND1", "OPERAND2")
        };
        let left = self.expr(block.inputs.get(left), written, active)?;
        let right = self.expr(block.inputs.get(right), written, active)?;
        if matches!(opcode, "operator_and" | "operator_or") {
            let op = if opcode == "operator_and" { "&&" } else { "||" };
            // Scratch evaluates both reporter inputs before applying the
            // boolean operator. Rust's && and || alone would skip the right
            // input, which can itself read changing state or have effects.
            return Some(NumericExpr {
                code: format!(
                    "{{ let left = {}; let right = {}; left {op} right }}",
                    boolean(left),
                    boolean(right)
                ),
                kind: Kind::Boolean,
            });
        }
        if left.kind != Kind::Number || right.kind != Kind::Number {
            return None;
        }
        if let Some(op) = match opcode {
            "operator_add" => Some("+"),
            "operator_subtract" => Some("-"),
            "operator_multiply" => Some("*"),
            "operator_divide" => Some("/"),
            _ => None,
        } {
            return Some(NumericExpr {
                code: format!(
                    "(scratch_number_operand({}) {op} scratch_number_operand({}))",
                    left.code, right.code
                ),
                kind: Kind::Number,
            });
        }
        let (op, fallback) = match opcode {
            "operator_equals" => ("==", "scratch_equals"),
            "operator_gt" => (">", "scratch_greater_than"),
            "operator_lt" => ("<", "scratch_less_than"),
            _ => return None,
        };
        // Scratch falls back to text comparison for NaN. Canonical guards make
        // the numeric value's text identical to the source value's text.
        Some(NumericExpr {
            code: format!(
                "{{ let left = {}; let right = {}; if left.is_nan() || right.is_nan() {{ {fallback}(left, right) }} else {{ left {op} right }} }}",
                left.code, right.code
            ),
            kind: Kind::Boolean,
        })
    }

    fn chain(
        &mut self,
        mut id: Option<&'a str>,
        written: &mut HashSet<usize>,
        out: &mut String,
        indent: usize,
        seen: &mut HashSet<String>,
    ) -> Option<()> {
        while let Some(current) = id {
            if !seen.insert(current.to_owned()) {
                return None;
            }
            let block = self.target.blocks.get(current)?;
            self.statements += 1;
            match block.opcode.as_str() {
                "control_if" | "control_if_else" => {
                    let condition =
                        self.expr(block.inputs.get("CONDITION"), written, &mut HashSet::new())?;
                    line(out, indent, &format!("if {} {{", boolean(condition)));
                    let branch = |name| {
                        block
                            .inputs
                            .get(name)
                            .and_then(parse_input)
                            .and_then(|value| match value {
                                InputValue::BlockReference(id) => Some(id),
                                _ => None,
                            })
                    };
                    let mut yes = written.clone();
                    self.chain(branch("SUBSTACK"), &mut yes, out, indent + 1, seen)?;
                    let mut no = written.clone();
                    if block.opcode == "control_if_else" {
                        line(out, indent, "} else {");
                        self.chain(branch("SUBSTACK2"), &mut no, out, indent + 1, seen)?;
                    }
                    line(out, indent, "}");
                    *written = yes.intersection(&no).copied().collect();
                }
                "control_stop"
                    if field_value(block, "STOP_OPTION").and_then(Value::as_str)
                        == Some("this script") =>
                {
                    self.emit_deferred_commits(out, indent);
                    line(out, indent, "return;")
                }
                "data_setvariableto" => {
                    let variable = variable_field(block, "VARIABLE", self.variables)?;
                    let input = block.inputs.get("VALUE");
                    let value = self.expr(input, written, &mut HashSet::new())?;
                    if value.kind != Kind::Number {
                        return None;
                    }
                    self.all.insert(variable.slot, variable.clone());
                    let direct_copy = match input.and_then(parse_input)? {
                        // Variable reads have already been guarded and cached
                        // as numeric locals by this specialization. Copy that
                        // native value directly instead of rebuilding a
                        // ScratchValue from state.get(). Literal strings and
                        // non-variable argument reporters still need their
                        // original representation in the dynamic variant.
                        InputValue::String(_) => true,
                        InputValue::VariableReference(_) => false,
                        InputValue::BlockReference(id) => self
                            .target
                            .blocks
                            .get(id)
                            .is_some_and(|b| b.opcode.starts_with("argument_reporter_")),
                        _ => false,
                    };
                    self.assigned.insert(variable.slot, variable.clone());
                    if direct_copy {
                        self.dynamic_assignments.insert(variable.slot);
                        let source_variable = match input.and_then(parse_input) {
                            Some(InputValue::VariableReference(reference)) => {
                                super::project::variable_for_target(
                                    self.target,
                                    reference.id,
                                    self.variables,
                                )
                                .cloned()
                            }
                            Some(InputValue::BlockReference(id)) => {
                                self.target.blocks.get(id).and_then(|block| {
                                    (block.opcode == "data_variable")
                                        .then(|| variable_field(block, "VARIABLE", self.variables))
                                        .flatten()
                                })
                            }
                            _ => None,
                        };
                        if let Some(source_variable) = source_variable {
                            self.dynamic_assignments.insert(source_variable.slot);
                        }
                    } else {
                        self.number_assignments.insert(variable.slot);
                    }
                    line(out, indent, &format!("let numeric_new = {};", value.code));
                    if direct_copy {
                        let source = expression(
                            self.target,
                            input,
                            self.variables,
                            self.lists,
                            &self.argument_values,
                            &mut HashSet::new(),
                        );
                        let setter = if variable.is_stage {
                            "set_shared"
                        } else {
                            "set_local_value"
                        };
                        line(
                            out,
                            indent,
                            &format!(
                                "let scratch_new_value: ScratchValue = ({source}).to_owned().into(); state.{setter}({slot}, scratch_new_value);",
                                slot = variable.slot,
                                setter = setter
                            ),
                        );
                    } else {
                        let setter = if variable.is_stage {
                            "set_shared_number"
                        } else {
                            "set_existing_local_number"
                        };
                        line(
                            out,
                            indent,
                            &format!(
                                "if scratch_defer_writes {{ scratch_dirty_{slot} = true; }} else {{ state.{setter}({slot}, numeric_new); }}",
                                slot = variable.slot,
                                setter = setter
                            ),
                        );
                    }
                    line(
                        out,
                        indent,
                        &format!("numeric_{} = numeric_new;", variable.slot),
                    );
                    written.insert(variable.slot);
                }
                "procedures_call" => {
                    let callee = procedure_for_call(block, self.procedures)?;
                    // Zero-argument nested procedures can be safely folded
                    // into this numeric region. Argument-bearing calls need
                    // their own type proof and remain on the dynamic path.
                    if !callee.argument_ids.is_empty() {
                        return None;
                    }
                    let callee_id = callee.definition_id.clone();
                    let Some(definition) = self.target.blocks.get(&callee_id) else {
                        return None;
                    };
                    let next = definition.next.as_deref();
                    let previous_arguments = std::mem::take(&mut self.arguments);
                    let previous_argument_values = std::mem::take(&mut self.argument_values);
                    let mut nested_written = written.clone();
                    line(out, indent, "{");
                    let nested = self.chain(next, &mut nested_written, out, indent + 1, seen);
                    line(out, indent, "}");
                    self.arguments = previous_arguments;
                    self.argument_values = previous_argument_values;
                    *written = nested_written;
                    nested?;
                }
                "motion_gotoxy" => {
                    let x = self.expr(block.inputs.get("X"), written, &mut HashSet::new())?;
                    let y = self.expr(block.inputs.get("Y"), written, &mut HashSet::new())?;
                    if x.kind != Kind::Number || y.kind != Kind::Number {
                        return None;
                    }
                    line(
                        out,
                        indent,
                        &format!(
                            "state.set_xy(scratch_number_operand({}), scratch_number_operand({}));",
                            x.code, y.code
                        ),
                    );
                }
                "pen_penDown" => line(out, indent, "state.pen_down();"),
                "pen_penUp" => line(out, indent, "state.pen_up();"),
                _ => return None,
            }
            id = block.next.as_deref();
        }
        Some(())
    }
}

fn boolean(expr: NumericExpr) -> String {
    match expr.kind {
        Kind::Boolean => expr.code,
        Kind::Number => format!("scratch_bool({})", expr.code),
    }
}

pub(super) fn generate_numeric_direct(
    target: &ScratchTarget,
    procedure: &ProcedureSignature,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    procedures: &[ProcedureSignature],
    name: &str,
    numeric_args_only: bool,
    proven_numeric_arguments: Option<&[bool]>,
) -> Option<String> {
    let args = procedure
        .argument_names
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), index))
        .collect();
    let argument_is_numeric = |index: usize| {
        numeric_args_only
            || proven_numeric_arguments
                .and_then(|arguments| arguments.get(index))
                .copied()
                .unwrap_or(false)
    };
    let argument_values = procedure
        .argument_names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            (
                name.clone(),
                if argument_is_numeric(index) {
                    format!("ScratchValue::Number(numeric_argument_{index})")
                } else {
                    format!("argument_{index}")
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let mut compiler = Compiler {
        target,
        variables,
        argument_values,
        lists,
        procedures,
        arguments: args,
        all: BTreeMap::new(),
        needed: BTreeMap::new(),
        assigned: BTreeMap::new(),
        number_assignments: HashSet::new(),
        dynamic_assignments: HashSet::new(),
        statements: 0,
    };
    let definition = target.blocks.get(&procedure.definition_id)?;
    let mut body = String::new();
    compiler.chain(
        definition.next.as_deref(),
        &mut HashSet::new(),
        &mut body,
        1,
        &mut HashSet::new(),
    )?;
    // Avoid creating extra versions of tiny dynamic procedures. A proven
    // numeric entry path can replace the generic call overhead even for a
    // small body, so keep that specialization available at every size.
    let arguments_are_proven_numeric = numeric_args_only
        || (!procedure.argument_ids.is_empty()
            && (0..procedure.argument_ids.len()).all(|index| argument_is_numeric(index)));
    if compiler.statements < 12 && !arguments_are_proven_numeric {
        return None;
    }
    let mut argument_operand_caches = Vec::new();
    for index in 0..procedure.argument_ids.len() {
        let operand = format!("scratch_number_operand(numeric_argument_{index})");
        if body.matches(&operand).count() > 1 {
            body = body.replace(&operand, &format!("scratch_operand_argument_{index}"));
            argument_operand_caches.push(index);
        }
    }
    let mut variable_operand_caches = Vec::new();
    for slot in compiler.all.keys() {
        let operand = format!("scratch_number_operand(numeric_{slot})");
        if body.matches(&operand).count() > 1 {
            body = body.replace(&operand, &format!("scratch_operand_{slot}"));
            body = body.replace(
                &format!("numeric_{slot} = numeric_new;"),
                &format!(
                    "numeric_{slot} = numeric_new; scratch_operand_{slot} = scratch_number_operand(numeric_new);"
                ),
            );
            variable_operand_caches.push(*slot);
        }
    }
    let params = (0..procedure.argument_ids.len())
        .map(|i| {
            if argument_is_numeric(i) {
                format!(", numeric_argument_{i}: f64")
            } else {
                format!(", argument_{i}: ScratchValue")
            }
        })
        .collect::<String>();
    let fallback_args = (0..procedure.argument_ids.len())
        .map(|i| {
            if argument_is_numeric(i) {
                format!(", ScratchValue::Number(numeric_argument_{i})")
            } else {
                format!(", argument_{i}")
            }
        })
        .collect::<String>();
    let fallback = format!("{name}_dynamic(state, clock{fallback_args}); return;");
    let mut output = String::new();
    line(
        &mut output,
        0,
        "#[allow(dead_code, unused_variables, unused_mut, unused_assignments, unreachable_code, unused_parens)]",
    );
    let function = if numeric_args_only {
        format!("{name}_numbers")
    } else {
        name.to_owned()
    };
    line(
        &mut output,
        0,
        &format!("fn {function}(state: &mut ScratchState, clock: &dyn Clock{params}) {{"),
    );
    if !numeric_args_only {
        for i in 0..procedure.argument_ids.len() {
            if !argument_is_numeric(i) {
                line(
                    &mut output,
                    1,
                    &format!(
                        "let Some(numeric_argument_{i}) = argument_{i}.canonical_number() else {{ {fallback} }};"
                    ),
                );
            }
        }
    }
    for (slot, variable) in &compiler.all {
        if compiler.needed.contains_key(slot) {
            if variable.numeric {
                line(
                    &mut output,
                    1,
                    &format!(
                        "let mut numeric_{slot} = {}.number_or_nan();",
                        variable.read_expression()
                    ),
                );
            } else {
                if variable.canonical_numeric {
                    line(
                        &mut output,
                        1,
                        &format!(
                            "let mut numeric_{slot} = scratch_to_number({});",
                            variable.read_expression()
                        ),
                    );
                } else {
                    line(
                        &mut output,
                        1,
                        &format!(
                            "let Some(mut numeric_{slot}) = {}.canonical_number() else {{ {fallback} }};",
                            variable.read_expression()
                        ),
                    );
                }
            }
        } else {
            line(
                &mut output,
                1,
                &format!("let mut numeric_{slot} = 0.0_f64;"),
            );
        }
    }
    for index in &argument_operand_caches {
        line(
            &mut output,
            1,
            &format!(
                "let scratch_operand_argument_{index} = scratch_number_operand(numeric_argument_{index});"
            ),
        );
    }
    for slot in &variable_operand_caches {
        line(
            &mut output,
            1,
            &format!("let mut scratch_operand_{slot} = scratch_number_operand(numeric_{slot});"),
        );
    }
    let deferred_slots = compiler.deferred_slots();
    let runtime_deferred = deferred_slots
        .iter()
        .copied()
        .filter(|slot| {
            compiler
                .assigned
                .get(slot)
                .is_some_and(|variable| !variable.monitor_visible)
        })
        .collect::<std::collections::BTreeSet<_>>();
    if runtime_deferred.is_empty() {
        line(&mut output, 1, "let scratch_defer_writes = false;");
    } else {
        line(
            &mut output,
            1,
            &format!(
                "let scratch_defer_writes = state.can_defer_variable_write_mask({});",
                variable_write_mask(runtime_deferred.iter().copied())
            ),
        );
    }
    for slot in compiler.assigned.keys() {
        line(
            &mut output,
            1,
            &format!(
                "let scratch_defer_{slot} = {};",
                if runtime_deferred.contains(slot) {
                    "scratch_defer_writes"
                } else {
                    "false"
                }
            ),
        );
    }
    for slot in compiler.assigned.keys() {
        line(
            &mut output,
            1,
            &format!("let mut scratch_dirty_{slot} = false;"),
        );
    }
    output.push_str(&body);
    compiler.emit_deferred_commits(&mut output, 1);
    line(&mut output, 0, "}\n");
    Some(output)
}
