//! Guarded numeric locals for bounded, non-suspending direct procedures.
//!
//! The dynamic procedure remains authoritative for noncanonical inputs. All
//! writes are committed immediately; only reads within the call are cached.

use super::{
    expressions::expression,
    literals::{field_value, line, number_literal},
    project::{ListInfo, VariableInfo},
    scripts::ProcedureSignature,
    statements::variable_field,
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

#[derive(Clone, Default)]
struct WriteState {
    definite: HashSet<usize>,
    possible: HashSet<usize>,
}

struct Compiler<'a> {
    target: &'a ScratchTarget,
    variables: &'a HashMap<String, VariableInfo>,
    argument_values: &'a HashMap<String, String>,
    lists: &'a HashMap<String, ListInfo>,
    arguments: HashMap<String, usize>,
    all: BTreeMap<usize, VariableInfo>,
    needed: BTreeMap<usize, VariableInfo>,
    assigned: BTreeMap<usize, VariableInfo>,
    numeric_reads: HashSet<usize>,
    number_assignments: HashSet<usize>,
    dynamic_assignments: HashSet<usize>,
    deferred: HashSet<usize>,
    dirty: HashSet<usize>,
    flush_states: Vec<WriteState>,
    statements: usize,
}

impl<'a> Compiler<'a> {
    fn deferred_slots(&self) -> HashSet<usize> {
        self.deferred.clone()
    }

    fn emit_deferred_commits(&self, out: &mut String, indent: usize, writes: &WriteState) {
        for variable in self.assigned.values().filter(|variable| {
            self.deferred.contains(&variable.slot) && writes.possible.contains(&variable.slot)
        }) {
            let setter = if variable.is_stage {
                "set_shared_number"
            } else {
                "set_local_number"
            };
            let condition = if writes.definite.contains(&variable.slot) {
                format!("scratch_defer_{}", variable.slot)
            } else {
                format!(
                    "scratch_defer_{} && scratch_dirty_{}",
                    variable.slot, variable.slot
                )
            };
            line(
                out,
                indent,
                &format!(
                    "if {condition} {{ state.{setter}({}, numeric_{}); }}",
                    variable.slot, variable.slot
                ),
            );
        }
    }

    fn read_variable(&mut self, variable: VariableInfo, written: &HashSet<usize>) -> NumericExpr {
        self.all.insert(variable.slot, variable.clone());
        self.numeric_reads.insert(variable.slot);
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
        possible_written: &mut HashSet<usize>,
        out: &mut String,
        indent: usize,
        seen: &mut HashSet<String>,
    ) -> Option<bool> {
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
                    let mut yes_possible = possible_written.clone();
                    let yes_reaches = self.chain(
                        branch("SUBSTACK"),
                        &mut yes,
                        &mut yes_possible,
                        out,
                        indent + 1,
                        seen,
                    )?;
                    let mut no = written.clone();
                    let mut no_possible = possible_written.clone();
                    if block.opcode == "control_if_else" {
                        line(out, indent, "} else {");
                        let no_reaches = self.chain(
                            branch("SUBSTACK2"),
                            &mut no,
                            &mut no_possible,
                            out,
                            indent + 1,
                            seen,
                        )?;
                        line(out, indent, "}");
                        match (yes_reaches, no_reaches) {
                            (true, true) => {
                                *written = yes.intersection(&no).copied().collect();
                                *possible_written =
                                    yes_possible.union(&no_possible).copied().collect();
                            }
                            (true, false) => {
                                *written = yes;
                                *possible_written = yes_possible;
                            }
                            (false, true) => {
                                *written = no;
                                *possible_written = no_possible;
                            }
                            (false, false) => return Some(false),
                        }
                    } else {
                        line(out, indent, "}");
                        // The implicit else path reaches the continuation unchanged.
                        if yes_reaches {
                            *written = yes.intersection(written).copied().collect();
                            *possible_written =
                                yes_possible.union(possible_written).copied().collect();
                        }
                    }
                }
                "control_stop"
                    if field_value(block, "STOP_OPTION").and_then(Value::as_str)
                        == Some("this script") =>
                {
                    let writes = WriteState {
                        definite: written.clone(),
                        possible: possible_written.clone(),
                    };
                    self.flush_states.push(writes.clone());
                    self.emit_deferred_commits(out, indent, &writes);
                    line(out, indent, "return;");
                    return Some(false);
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
                            self.argument_values,
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
                            "set_local_number"
                        };
                        if self.deferred.contains(&variable.slot) {
                            let dirty = if self.dirty.contains(&variable.slot) {
                                format!(" scratch_dirty_{} = true;", variable.slot)
                            } else {
                                String::new()
                            };
                            line(
                                out,
                                indent,
                                &format!(
                                    "if scratch_defer_{slot} {{ numeric_{slot} = numeric_new;{dirty} }} else {{ state.{setter}({slot}, numeric_new); }}",
                                    slot = variable.slot,
                                    setter = setter
                                ),
                            );
                        } else {
                            // This slot also receives a value with dynamic Scratch
                            // representation elsewhere in the procedure. Deferral
                            // is statically unsafe, so emit only the immediate path.
                            line(
                                out,
                                indent,
                                &format!("state.{setter}({}, numeric_new);", variable.slot),
                            );
                        }
                    }
                    if self.numeric_reads.contains(&variable.slot)
                        || self.deferred.contains(&variable.slot)
                    {
                        line(
                            out,
                            indent,
                            &format!("numeric_{} = numeric_new;", variable.slot),
                        );
                    }
                    written.insert(variable.slot);
                    possible_written.insert(variable.slot);
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
        Some(true)
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
    name: &str,
    numeric_args_only: bool,
) -> Option<String> {
    let args: HashMap<String, usize> = procedure
        .argument_names
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), index))
        .collect();
    let argument_values = procedure
        .argument_names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            (
                name.clone(),
                if numeric_args_only {
                    format!("ScratchValue::Number(numeric_argument_{index})")
                } else {
                    format!("argument_{index}")
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let make_compiler = |deferred: HashSet<usize>, dirty: HashSet<usize>| Compiler {
        target,
        variables,
        argument_values: &argument_values,
        lists,
        arguments: args.clone(),
        all: BTreeMap::new(),
        needed: BTreeMap::new(),
        assigned: BTreeMap::new(),
        numeric_reads: HashSet::new(),
        number_assignments: HashSet::new(),
        dynamic_assignments: HashSet::new(),
        deferred,
        dirty,
        flush_states: Vec::new(),
        statements: 0,
    };
    let definition = target.blocks.get(&procedure.definition_id)?;
    // First walk the procedure as a control-flow graph. This computes which
    // assignments can be deferred and which flushes are definitely dirty,
    // before any optimized Rust is emitted.
    let mut analysis = make_compiler(HashSet::new(), HashSet::new());
    let mut analysis_body = String::new();
    let mut analysis_written = HashSet::new();
    let mut analysis_possible = HashSet::new();
    let reaches_exit = analysis.chain(
        definition.next.as_deref(),
        &mut analysis_written,
        &mut analysis_possible,
        &mut analysis_body,
        1,
        &mut HashSet::new(),
    )?;
    if reaches_exit {
        analysis.flush_states.push(WriteState {
            definite: analysis_written,
            possible: analysis_possible,
        });
    }
    // Avoid creating extra versions of tiny procedures. Numeric calculation
    // and branch-only procedures benefit too; they need not contain motion.
    if analysis.statements < 12 {
        return None;
    }
    let deferred = analysis
        .number_assignments
        .difference(&analysis.dynamic_assignments)
        .copied()
        .collect::<HashSet<_>>();
    let mut dirty = HashSet::new();
    for flush in &analysis.flush_states {
        dirty.extend(
            deferred
                .iter()
                .filter(|slot| flush.possible.contains(slot) && !flush.definite.contains(slot))
                .copied(),
        );
    }

    let mut compiler = make_compiler(deferred, dirty);
    let mut body = String::new();
    let mut written = HashSet::new();
    let mut possible_written = HashSet::new();
    let reaches_exit = compiler.chain(
        definition.next.as_deref(),
        &mut written,
        &mut possible_written,
        &mut body,
        1,
        &mut HashSet::new(),
    )?;
    let params = (0..procedure.argument_ids.len())
        .map(|i| {
            if numeric_args_only {
                format!(", numeric_argument_{i}: f64")
            } else {
                format!(", argument_{i}: ScratchValue")
            }
        })
        .collect::<String>();
    let fallback_args = (0..procedure.argument_ids.len())
        .map(|i| {
            if numeric_args_only {
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
            line(
                &mut output,
                1,
                &format!(
                    "let Some(numeric_argument_{i}) = argument_{i}.canonical_number() else {{ {fallback} }};"
                ),
            );
        }
    }
    let deferred_slots = compiler.deferred_slots();
    for (slot, variable) in &compiler.all {
        if !compiler.numeric_reads.contains(slot) && !deferred_slots.contains(slot) {
            continue;
        }
        if compiler.needed.contains_key(slot) {
            line(
                &mut output,
                1,
                &format!(
                    "let Some(mut numeric_{slot}) = {}.canonical_number() else {{ {fallback} }};",
                    variable.read_expression()
                ),
            );
        } else {
            line(
                &mut output,
                1,
                &format!("let mut numeric_{slot} = 0.0_f64;"),
            );
        }
    }
    for slot in &deferred_slots {
        if compiler.assigned.contains_key(slot) {
            line(
                &mut output,
                1,
                &format!("let scratch_defer_{slot} = state.can_defer_variable_writes(&[{slot}]);"),
            );
        }
    }
    for slot in &compiler.dirty {
        line(
            &mut output,
            1,
            &format!("let mut scratch_dirty_{slot} = false;"),
        );
    }
    output.push_str(&body);
    if reaches_exit {
        compiler.emit_deferred_commits(
            &mut output,
            1,
            &WriteState {
                definite: written,
                possible: possible_written,
            },
        );
    }
    line(&mut output, 0, "}\n");
    Some(output)
}
