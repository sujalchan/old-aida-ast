use super::*;

pub(super) fn generate_resumable_statement(
    context: StatementContext<'_>,
    block: &ScratchBlock,
    next_pc: usize,
    output: &mut String,
    indent: usize,
) {
    let StatementContext {
        target,
        variables,
        lists,
        arguments,
    } = context;
    match block.opcode.as_str() {
        // scratch vm keeps these imported legacy commands as no-ops.
        "looks_hideallsprites"
        | "looks_changestretchby"
        | "looks_setstretchto"
        | "motion_scroll_right"
        | "motion_scroll_up"
        | "motion_align_scene" => line(output, indent, "let _ = ();"),
        "looks_show" => line(output, indent, "state.set_visible(true);"),
        "looks_hide" => line(output, indent, "state.set_visible(false);"),
        "looks_switchcostumeto" => {
            let costume = owned_input(
                target,
                block.inputs.get("COSTUME"),
                variables,
                lists,
                arguments,
            );
            line(
                output,
                indent,
                &format!("state.switch_costume_to({costume});"),
            );
        }
        "looks_nextcostume" => line(output, indent, "state.next_costume();"),
        "looks_changesizeby" | "looks_setsizeto" => {
            let input_name = if block.opcode == "looks_changesizeby" {
                "CHANGE"
            } else {
                "SIZE"
            };
            let size = expression(
                target,
                block.inputs.get(input_name),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            let method = if block.opcode == "looks_changesizeby" {
                "change_size_by"
            } else {
                "set_size"
            };
            line(
                output,
                indent,
                &format!(
                    "let scratch_size_input = scratch_to_number({size}); state.{method}(scratch_size_input);"
                ),
            );
        }
        "pen_clear" => line(output, indent, "state.clear_pen();"),
        "sound_play" => {
            let selector = owned_input(
                target,
                block.inputs.get("SOUND_MENU"),
                variables,
                lists,
                arguments,
            );
            line(
                output,
                indent,
                &format!(
                    "let scratch_sound_selector: ScratchValue = {selector}; state.start_sound(scratch_sound_selector, clock.now_millis());"
                ),
            );
        }
        "sound_stopallsounds" => line(output, indent, "state.stop_all_sounds(clock.now_millis());"),
        "sound_cleareffects" => line(
            output,
            indent,
            "state.clear_sound_effects(clock.now_millis());",
        ),
        "pen_penDown" => line(output, indent, "state.pen_down();"),
        "pen_penUp" => line(output, indent, "state.pen_up();"),
        "pen_stamp" => line(output, indent, "state.stamp();"),
        "pen_setPenColorToColor" => {
            let color_expression = expression(
                target,
                block.inputs.get("COLOR"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            // State-backed reporters cannot be borrowed across a mutable state
            // receiver; local and literal values can be passed through directly.
            let color = if color_expression.starts_with("state.") {
                owned_input(
                    target,
                    block.inputs.get("COLOR"),
                    variables,
                    lists,
                    arguments,
                )
            } else {
                color_expression
            };
            let setter = if target_has_pen_hsv_controls(target) {
                format!("state.set_pen_rgb_color({color});")
            } else {
                format!("state.set_pen_rgb_color_without_hsv({color});")
            };
            line(output, indent, &setter);
        }
        "pen_changePenColorParamBy" | "pen_setPenColorParamTo" => {
            let parameter = pen_color_parameter_input(
                target,
                block.inputs.get("COLOR_PARAM"),
                variables,
                lists,
                arguments,
            );
            let value = expression(
                target,
                block.inputs.get("VALUE"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            line(
                output,
                indent,
                &format!(
                    "state.set_pen_color_parameter({parameter}, scratch_to_number({value}), {});",
                    block.opcode == "pen_changePenColorParamBy"
                ),
            );
        }
        "pen_setPenHueToNumber" | "pen_changePenHueBy" => {
            let value = expression(
                target,
                block.inputs.get("HUE"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            let method = if block.opcode == "pen_setPenHueToNumber" {
                "set_legacy_pen_hue"
            } else {
                "change_legacy_pen_hue"
            };
            line(
                output,
                indent,
                &format!("state.{method}(scratch_to_number({value}));"),
            );
        }
        "pen_setPenShadeToNumber" | "pen_changePenShadeBy" => {
            let value = expression(
                target,
                block.inputs.get("SHADE"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            let method = if block.opcode == "pen_setPenShadeToNumber" {
                "set_legacy_pen_shade"
            } else {
                "change_legacy_pen_shade"
            };
            line(
                output,
                indent,
                &format!("state.{method}(scratch_to_number({value}));"),
            );
        }
        "pen_setPenSizeTo" | "pen_changePenSizeBy" => {
            let value = expression(
                target,
                block.inputs.get("SIZE"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            let method = if block.opcode == "pen_setPenSizeTo" {
                "set_pen_size"
            } else {
                "change_pen_size"
            };
            line(
                output,
                indent,
                &format!("state.{method}(scratch_to_number({value}));"),
            );
        }
        "sensing_resettimer" => line(
            output,
            indent,
            "state.reset_project_timer(clock.now_millis());",
        ),
        "event_broadcast" | "event_broadcastandwait" => {
            let message = broadcast_message_expression(
                target,
                block.inputs.get("BROADCAST_INPUT"),
                variables,
                lists,
                arguments,
            );
            line(
                output,
                indent,
                &format!(
                    "script.frames.last_mut().unwrap().pc = {};",
                    pc_literal(next_pc)
                ),
            );
            line(
                output,
                indent,
                &format!(
                    "return ScriptStatus::Broadcast {{ message: broadcast_message_key(scratch_string({message})), wait: {} }};",
                    block.opcode == "event_broadcastandwait"
                ),
            );
        }
        "control_stop" => match field_value(block, "STOP_OPTION").and_then(Value::as_str) {
            Some("all") => line(output, indent, "return ScriptStatus::StopAll;"),
            Some("this script") => {
                line(
                    output,
                    indent,
                    "if script.frames.len() > 1 { script.pop_frame(); continue; } return ScriptStatus::Complete;",
                );
            }
            Some("other scripts in sprite") | Some("other scripts in stage") => {
                line(
                    output,
                    indent,
                    &format!(
                        "script.frames.last_mut().unwrap().pc = {};",
                        pc_literal(next_pc)
                    ),
                );
                line(output, indent, "return ScriptStatus::StopOtherScripts;");
            }
            _ => {}
        },
        // use mutation results instead of retaining an arc snapshot across each generated write.
        "data_addtolist" => {
            if let Some(list) = list_field(block, "LIST", lists) {
                let input = block.inputs.get("ITEM");
                let item = owned_input(target, input, variables, lists, arguments);
                line(
                    output,
                    indent,
                    &format!("let scratch_list_item: ScratchValue = {item};"),
                );
                line(
                    output,
                    indent,
                    &format!(
                        "let scratch_list_changed = state.list_mut({}).add(scratch_list_item);",
                        list.slot
                    ),
                );
                line(
                    output,
                    indent,
                    &format!(
                        "state.list_changed_result({}, scratch_list_changed);",
                        list.slot
                    ),
                );
            } else {
                line(output, indent, "todo!(\"missing list reference\");");
            }
        }
        "data_deleteoflist" => {
            if let Some(list) = list_field(block, "LIST", lists) {
                let index = owned_input(
                    target,
                    block.inputs.get("INDEX"),
                    variables,
                    lists,
                    arguments,
                );
                line(
                    output,
                    indent,
                    &format!("let scratch_list_index: ScratchValue = {index};"),
                );
                line(
                    output,
                    indent,
                    &format!(
                        "let scratch_list_changed = state.list_mut({}).delete(scratch_list_index);",
                        list.slot
                    ),
                );
                line(
                    output,
                    indent,
                    &format!(
                        "state.list_changed_result({}, scratch_list_changed);",
                        list.slot
                    ),
                );
            } else {
                line(output, indent, "todo!(\"missing list reference\");");
            }
        }
        "data_deletealloflist" => {
            if let Some(list) = list_field(block, "LIST", lists) {
                line(
                    output,
                    indent,
                    &format!(
                        "let scratch_list_changed = state.list_mut({}).delete_all();",
                        list.slot
                    ),
                );
                line(
                    output,
                    indent,
                    &format!(
                        "state.list_changed_result({}, scratch_list_changed);",
                        list.slot
                    ),
                );
            } else {
                line(output, indent, "todo!(\"missing list reference\");");
            }
        }
        "data_insertatlist" | "data_replaceitemoflist" => {
            if let Some(list) = list_field(block, "LIST", lists) {
                let input = block.inputs.get("ITEM");
                let index_input = block.inputs.get("INDEX");
                let numeric_variables = (|| {
                    let index = variable_input_info(target, index_input, variables)?;
                    let item = variable_input_info(target, input, variables)?;
                    (index.numeric && item.numeric).then_some((index, item))
                })();
                if block.opcode == "data_replaceitemoflist"
                    && let Some((index, item)) = numeric_variables
                {
                    line(
                        output,
                        indent,
                        &format!(
                            "let scratch_list_numeric_pair = match ({}, {}) {{ (ScratchValue::Number(scratch_list_index), ScratchValue::Number(scratch_list_item)) => Some((*scratch_list_index, *scratch_list_item)), _ => None }};",
                            index.read_expression(),
                            item.read_expression()
                        ),
                    );
                    line(
                        output,
                        indent,
                        &format!(
                            "let scratch_list_changed = if let Some((scratch_list_index, scratch_list_item)) = scratch_list_numeric_pair {{ state.list_mut({}).replace_number(scratch_list_index, scratch_list_item) }} else {{ let scratch_list_index: ScratchValue = ({}).clone(); let scratch_list_item: ScratchValue = ({}).clone(); state.list_mut({}).replace(scratch_list_index, scratch_list_item) }};",
                            list.slot,
                            index.read_expression(),
                            item.read_expression(),
                            list.slot
                        ),
                    );
                } else {
                    let item = owned_input(target, input, variables, lists, arguments);
                    let index = owned_input(target, index_input, variables, lists, arguments);
                    line(
                        output,
                        indent,
                        &format!("let scratch_list_item: ScratchValue = {item};"),
                    );
                    line(
                        output,
                        indent,
                        &format!("let scratch_list_index: ScratchValue = {index};"),
                    );
                    if block.opcode == "data_insertatlist" {
                        line(
                            output,
                            indent,
                            &format!(
                                "let scratch_list_changed = state.list_mut({}).insert(scratch_list_item, scratch_list_index);",
                                list.slot
                            ),
                        );
                    } else {
                        line(
                            output,
                            indent,
                            &format!(
                                "let scratch_list_changed = state.list_mut({}).replace(scratch_list_index, scratch_list_item);",
                                list.slot
                            ),
                        );
                    }
                }
                line(
                    output,
                    indent,
                    &format!(
                        "state.list_changed_result({}, scratch_list_changed);",
                        list.slot
                    ),
                );
            } else {
                line(output, indent, "todo!(\"missing list reference\");");
            }
        }
        "data_showvariable" | "data_hidevariable" => {
            if let Some(variable) = variable_field(block, "VARIABLE", variables) {
                line(
                    output,
                    indent,
                    &format!(
                        "state.set_variable_visible({}, {});",
                        variable.slot,
                        block.opcode == "data_showvariable"
                    ),
                );
            } else {
                line(output, indent, "todo!(\"missing variable reference\");");
            }
        }
        "data_showlist" | "data_hidelist" => {
            if let Some(list) = list_field(block, "LIST", lists) {
                line(
                    output,
                    indent,
                    &format!(
                        "state.set_list_visible({}, {});",
                        list.slot,
                        block.opcode == "data_showlist"
                    ),
                );
            } else {
                line(output, indent, "todo!(\"missing list reference\");");
            }
        }
        "data_setvariableto" => {
            if let Some(variable) = variable_field(block, "VARIABLE", variables) {
                let input = block.inputs.get("VALUE");
                let value = expression(
                    target,
                    input,
                    variables,
                    lists,
                    arguments,
                    &mut HashSet::new(),
                );
                if variable.numeric {
                    let setter = if variable.is_stage {
                        "set_shared_number"
                    } else {
                        "set_local_number"
                    };
                    if let Some(source_variable) = variable_input_info(target, input, variables)
                        .filter(|source| source.numeric)
                    {
                        let source = source_variable.read_expression();
                        line(
                            output,
                            indent,
                            &format!(
                                "if let ScratchValue::Number(scratch_numeric_value) = {source} {{ state.{setter}({}, *scratch_numeric_value); }} else {{ let scratch_new_value: ScratchValue = ({source}).clone(); {} }}",
                                variable.slot,
                                variable.write_statement("scratch_new_value")
                            ),
                        );
                    } else {
                        line(
                            output,
                            indent,
                            &format!("state.{setter}({}, {value});", variable.slot),
                        );
                    }
                } else if let Some(source) =
                    variable_input(target, input, variables, lists, arguments)
                {
                    line(
                        output,
                        indent,
                        &format!("let scratch_new_value: ScratchValue = ({source}).clone();"),
                    );
                    line(
                        output,
                        indent,
                        &variable.write_statement("scratch_new_value"),
                    );
                } else {
                    line(
                        output,
                        indent,
                        &format!("let scratch_new_value: ScratchValue = ({value}).into();"),
                    );
                    line(
                        output,
                        indent,
                        &variable.write_statement("scratch_new_value"),
                    );
                }
            } else {
                line(output, indent, "todo!(\"missing variable reference\");");
            }
        }
        "data_changevariableby" => {
            if let Some(variable) = variable_field(block, "VARIABLE", variables) {
                let value = expression(
                    target,
                    block.inputs.get("VALUE"),
                    variables,
                    lists,
                    arguments,
                    &mut HashSet::new(),
                );
                line(
                    output,
                    indent,
                    &format!("let scratch_change = scratch_to_number({value});"),
                );
                // the runtime fast path falls back for dynamic values while keeping numeric monitor writes allocation-free.
                let method = if variable.is_stage {
                    "change_shared_number"
                } else {
                    "change_local_number"
                };
                line(
                    output,
                    indent,
                    &format!("state.{method}({}, scratch_change);", variable.slot),
                );
            } else {
                line(output, indent, "todo!(\"missing variable reference\");");
            }
        }
        "looks_switchbackdropto" => {
            let value = expression(
                target,
                block.inputs.get("BACKDROP"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            line(output, indent, &format!("state.switch_backdrop({value});"));
        }
        "looks_nextbackdrop" => line(output, indent, "state.next_backdrop();"),
        "looks_say" | "looks_think" => {
            let value = expression(
                target,
                block.inputs.get("MESSAGE"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            line(
                output,
                indent,
                &format!("let scratch_bubble_text = scratch_string({value});"),
            );
            if block.opcode == "looks_say" {
                let name = say_variable_name(target, block.inputs.get("MESSAGE"))
                    .unwrap_or_else(|| "say".to_owned());
                line(
                    output,
                    indent,
                    &format!(
                        "print_value!(state, {}, scratch_bubble_text.clone());",
                        quote(&name)
                    ),
                );
            }
            line(
                output,
                indent,
                &format!(
                    "state.set_speech_bubble(BubbleKind::{}, scratch_bubble_text, None);",
                    if block.opcode == "looks_say" {
                        "Say"
                    } else {
                        "Think"
                    }
                ),
            );
        }
        "looks_sayforsecs" | "looks_thinkforsecs" => {
            let value = expression(
                target,
                block.inputs.get("MESSAGE"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            let duration = expression(
                target,
                block.inputs.get("SECS"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            line(
                output,
                indent,
                &format!("let scratch_bubble_text = scratch_string({value});"),
            );
            if block.opcode == "looks_sayforsecs" {
                let name = say_variable_name(target, block.inputs.get("MESSAGE"))
                    .unwrap_or_else(|| "say".to_owned());
                line(
                    output,
                    indent,
                    &format!(
                        "print_value!(state, {}, scratch_bubble_text.clone());",
                        quote(&name)
                    ),
                );
            }
            line(
                output,
                indent,
                &format!(
                    "let duration_ms = (scratch_to_number({duration}).max(0.0) * 1000.0).ceil() as u64; let wait_deadline = clock.now_millis().saturating_add(duration_ms);"
                ),
            );
            line(
                output,
                indent,
                &format!(
                    "state.set_speech_bubble(BubbleKind::{}, scratch_bubble_text, Some(wait_deadline));",
                    if block.opcode == "looks_sayforsecs" {
                        "Say"
                    } else {
                        "Think"
                    }
                ),
            );
            line(
                output,
                indent,
                &format!(
                    "script.frames.last_mut().unwrap().pc = {};",
                    pc_literal(next_pc)
                ),
            );
            line(
                output,
                indent,
                "return ScriptStatus::WaitUntil(wait_deadline);",
            );
        }
        // lower motion through instance-owned state so fencing and rendering stay centralized.
        "motion_movesteps" => {
            let steps = expression(
                target,
                block.inputs.get("STEPS"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            line(
                output,
                indent,
                &format!("state.move_steps(scratch_to_number({steps}));"),
            );
        }
        "motion_setrotationstyle" => {
            let style = field_value(block, "STYLE")
                .and_then(Value::as_str)
                .unwrap_or("all around");
            line(
                output,
                indent,
                &format!("state.set_rotation_style({});", quote(style)),
            );
        }
        "motion_ifonedgebounce" => {
            line(output, indent, "state.bounce_on_edge();");
        }
        "motion_gotoxy" => {
            let x = expression(
                target,
                block.inputs.get("X"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            let y = expression(
                target,
                block.inputs.get("Y"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            line(
                output,
                indent,
                &format!("state.set_xy(scratch_to_number({x}), scratch_to_number({y}));"),
            );
        }
        "motion_setx" => {
            let x = expression(
                target,
                block.inputs.get("X"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            line(
                output,
                indent,
                &format!("state.set_x(scratch_to_number({x}));"),
            );
        }
        "motion_sety" => {
            let y = expression(
                target,
                block.inputs.get("Y"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            line(
                output,
                indent,
                &format!("state.set_y(scratch_to_number({y}));"),
            );
        }
        "motion_pointindirection" => {
            let direction = expression(
                target,
                block.inputs.get("DIRECTION"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            line(
                output,
                indent,
                &format!("state.set_direction(scratch_to_number({direction}));"),
            );
        }
        // apply turns to the scheduled instance and let runtime direction handling normalize the angle.
        "motion_turnright" | "motion_turnleft" => {
            let degrees = expression(
                target,
                block.inputs.get("DEGREES"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            let operator = if block.opcode == "motion_turnright" {
                "+"
            } else {
                "-"
            };
            line(
                output,
                indent,
                &format!(
                    "state.set_direction(state.direction() {operator} scratch_to_number({degrees}));"
                ),
            );
        }
        "motion_pointtowards" => {
            let random_target =
                block
                    .inputs
                    .get("TOWARDS")
                    .and_then(parse_input)
                    .and_then(|input| match input {
                        InputValue::BlockReference(id) => target
                            .blocks
                            .get(id)
                            .and_then(|menu| field_value(menu, "TOWARDS"))
                            .and_then(Value::as_str),
                        InputValue::String(value) | InputValue::Number(value) => value.as_str(),
                        _ => None,
                    })
                    == Some("_random_");
            if random_target {
                line(
                    output,
                    indent,
                    "state.set_direction(scratch_round(scratch_random(-180.0_f64, 180.0_f64)));",
                );
            } else if let Some(position) =
                motion_target_position_expression(target, block, "TOWARDS")
            {
                line(
                    output,
                    indent,
                    &format!(
                        "if let Some((target_x, target_y)) = {position} {{ if let Some((motion_x, motion_y)) = state.position(state.current_instance()) {{ state.set_direction(90.0 - (target_y - motion_y).atan2(target_x - motion_x).to_degrees()); }} }}"
                    ),
                );
            }
        }
        "motion_goto" => {
            if let Some(position) = motion_target_position_expression(target, block, "TO") {
                line(
                    output,
                    indent,
                    &format!("if let Some((motion_x, motion_y)) = {position} {{"),
                );
                line(output, indent + 1, "state.set_xy(motion_x, motion_y);");
                line(output, indent, "}");
            } else {
                line(output, indent, "todo!(\"missing motion target\");");
            }
        }
        "motion_changexby" => {
            let dx = expression(
                target,
                block.inputs.get("DX"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            line(
                output,
                indent,
                &format!("state.change_x_by(scratch_to_number({dx}));"),
            );
        }
        "motion_changeyby" => {
            let dy = expression(
                target,
                block.inputs.get("DY"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            line(
                output,
                indent,
                &format!("state.change_y_by(scratch_to_number({dy}));"),
            );
        }
        "motion_glideto" | "motion_glidesecstoxy" => generate_glide_statement(
            StatementContext {
                target,
                variables,
                lists,
                arguments,
            },
            block,
            next_pc,
            output,
            indent,
        ),
        "control_create_clone_of" => {
            let option = block
                .inputs
                .get("CLONE_OPTION")
                .and_then(parse_input)
                .and_then(|input| match input {
                    InputValue::BlockReference(id) => target
                        .blocks
                        .get(id)
                        .and_then(|menu| menu.fields.get("CLONE_OPTION"))
                        .and_then(Value::as_array)
                        .and_then(|field| field.first())
                        .and_then(Value::as_str),
                    InputValue::String(value) | InputValue::Number(value) => value.as_str(),
                    _ => None,
                })
                .unwrap_or("");
            if option == "_myself_" {
                line(
                    output,
                    indent,
                    "let clone_target = Some(state.current_target());",
                );
                line(
                    output,
                    indent,
                    "let clone_source = Some(state.current_instance());",
                );
            } else {
                line(
                    output,
                    indent,
                    &format!(
                        "let clone_target = state.clone_target_for_name({});",
                        quote(option)
                    ),
                );
                line(
                    output,
                    indent,
                    "let clone_source = clone_target.and_then(|target| state.original_instance(target));",
                );
            }
            line(
                output,
                indent,
                "if let (Some(target), Some(source)) = (clone_target, clone_source) { state.create_clone(source, target, 300); }",
            );
        }
        "control_delete_this_clone" => {
            line(
                output,
                indent,
                "if state.delete_current_clone() { return ScriptStatus::Complete; }",
            );
        }
        "procedures_call" => line(
            output,
            indent,
            "todo!(\"procedure calls are lowered as resumable frames\");",
        ),
        opcode => line(
            output,
            indent,
            &format!(
                "todo!({});",
                quote(&format!("unsupported statement: {opcode}"))
            ),
        ),
    }
}

pub(super) fn broadcast_message_expression(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    arguments: &HashMap<String, String>,
) -> String {
    if let Some(id) = input
        .and_then(Value::as_array)
        .and_then(|input| input.get(1))
        .and_then(Value::as_array)
        .and_then(|primitive| primitive.get(2))
        .and_then(Value::as_str)
        && let Some(message) = target.broadcasts.get(id)
    {
        return quote(message);
    }
    expression(
        target,
        input,
        variables,
        lists,
        arguments,
        &mut HashSet::new(),
    )
}

pub(super) fn motion_target_position_expression(
    target: &ScratchTarget,
    block: &ScratchBlock,
    input_name: &str,
) -> Option<String> {
    let option = block
        .inputs
        .get(input_name)
        .and_then(parse_input)
        .and_then(|input| match input {
            InputValue::BlockReference(id) => target
                .blocks
                .get(id)
                .and_then(|menu| {
                    field_value(
                        menu,
                        if input_name == "TOWARDS" {
                            "TOWARDS"
                        } else {
                            "TO"
                        },
                    )
                })
                .and_then(Value::as_str),
            InputValue::String(value) | InputValue::Number(value) => value.as_str(),
            _ => None,
        })?;

    Some(match option {
        "_random_" => "Some((scratch_round(scratch_random(-240.5_f64, 240.5_f64)), scratch_round(scratch_random(-179.5_f64, 180.5_f64))))".to_owned(),
        "_mouse_" => "Some((state.mouse_x(), state.mouse_y()))".to_owned(),
        name => format!(
            "state.clone_target_for_name({}).and_then(|target| state.original_instance(target)).and_then(|instance| state.position(instance))",
            quote(name)
        ),
    })
}

pub(super) fn generate_glide_statement(
    context: StatementContext<'_>,
    block: &ScratchBlock,
    next_pc: usize,
    output: &mut String,
    indent: usize,
) {
    let StatementContext {
        target,
        variables,
        lists,
        arguments,
    } = context;
    let seconds = expression(
        target,
        block.inputs.get("SECS"),
        variables,
        lists,
        arguments,
        &mut HashSet::new(),
    );
    line(
        output,
        indent,
        "if script.frames.last().unwrap().glide.is_none() {",
    );

    if block.opcode == "motion_glideto" {
        if let Some(position) = motion_target_position_expression(target, block, "TO") {
            line(
                output,
                indent + 1,
                &format!("if let Some((motion_end_x, motion_end_y)) = {position} {{"),
            );
            line(
                output,
                indent + 2,
                "let motion_start = state.position(state.current_instance()).unwrap_or((0.0, 0.0));",
            );
            line(
                output,
                indent + 2,
                &format!(
                    "script.frames.last_mut().unwrap().glide = Some(GlideState::new(clock.now_millis(), scratch_to_number({seconds}), motion_start, (motion_end_x, motion_end_y)));"
                ),
            );
            line(output, indent + 1, "} else {");
            line(
                output,
                indent + 2,
                &format!(
                    "script.frames.last_mut().unwrap().pc = {};",
                    pc_literal(next_pc)
                ),
            );
            line(output, indent + 2, "continue;");
            line(output, indent + 1, "}");
        } else {
            line(output, indent + 1, "todo!(\"missing glide target\");");
        }
    } else {
        let x = expression(
            target,
            block.inputs.get("X"),
            variables,
            lists,
            arguments,
            &mut HashSet::new(),
        );
        let y = expression(
            target,
            block.inputs.get("Y"),
            variables,
            lists,
            arguments,
            &mut HashSet::new(),
        );
        line(
            output,
            indent + 1,
            "let motion_start = state.position(state.current_instance()).unwrap_or((0.0, 0.0));",
        );
        line(
            output,
            indent + 1,
            &format!("let motion_destination = (scratch_to_number({x}), scratch_to_number({y}));"),
        );
        line(
            output,
            indent + 1,
            &format!(
                "script.frames.last_mut().unwrap().glide = Some(GlideState::new(clock.now_millis(), scratch_to_number({seconds}), motion_start, motion_destination));"
            ),
        );
    }
    line(output, indent, "}");
    line(
        output,
        indent,
        "let motion_glide = script.frames.last().unwrap().glide.unwrap();",
    );
    line(
        output,
        indent,
        "if let Some((motion_x, motion_y)) = motion_glide.position_at(clock.now_millis()) {",
    );
    line(output, indent + 1, "state.set_xy(motion_x, motion_y);");
    line(
        output,
        indent + 1,
        "if script.yield_at_safe_point(clock) { return ScriptStatus::Yield; }",
    );
    line(output, indent + 1, "continue;");
    line(output, indent, "} else {");
    line(
        output,
        indent + 1,
        "let (motion_x, motion_y) = motion_glide.destination();",
    );
    line(output, indent + 1, "state.set_xy(motion_x, motion_y);");
    line(
        output,
        indent + 1,
        "script.frames.last_mut().unwrap().glide = None;",
    );
    line(
        output,
        indent + 1,
        &format!(
            "script.frames.last_mut().unwrap().pc = {};",
            pc_literal(next_pc)
        ),
    );
    line(output, indent + 1, "continue;");
    line(output, indent, "}");
}

pub(in crate::rust) fn procedure_for_call<'a>(
    block: &ScratchBlock,
    procedures: &'a [ProcedureSignature],
) -> Option<&'a ProcedureSignature> {
    let proccode = block.mutation.as_object()?.get("proccode")?.as_str()?;
    procedures
        .iter()
        .find(|procedure| procedure.proccode == proccode)
}

// resolve direct variable inputs so assignments copy the current dynamic value.
pub(super) fn variable_input(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    arguments: &HashMap<String, String>,
) -> Option<String> {
    match input.and_then(parse_input)? {
        InputValue::VariableReference(reference) => {
            super::super::project::variable_for_target(target, reference.id, variables)
                .map(VariableInfo::read_expression)
        }
        InputValue::BlockReference(id) => {
            let reporter = target.blocks.get(id)?;
            if reporter.opcode == "data_variable" {
                variable_field(reporter, "VARIABLE", variables)
                    .map(|variable| variable.read_expression())
            } else if matches!(
                reporter.opcode.as_str(),
                "argument_reporter_string_number" | "argument_reporter_boolean"
            ) {
                field_value(reporter, "VALUE")
                    .and_then(Value::as_str)
                    .and_then(|name| arguments.get(name).cloned())
            } else if reporter.opcode == "data_itemoflist" {
                let list = list_field(reporter, "LIST", lists)?;
                let index = expression(
                    target,
                    reporter.inputs.get("INDEX"),
                    variables,
                    lists,
                    arguments,
                    &mut HashSet::new(),
                );
                Some(format!("{}.get({index})", list.read_expression()))
            } else {
                None
            }
        }
        _ => None,
    }
}

// return identity and static numeric proof for direct scalar variable inputs.
pub(super) fn variable_input_info(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
) -> Option<VariableInfo> {
    match input.and_then(parse_input)? {
        InputValue::VariableReference(reference) => {
            super::super::project::variable_for_target(target, reference.id, variables).cloned()
        }
        InputValue::BlockReference(id) => {
            let reporter = target.blocks.get(id)?;
            (reporter.opcode == "data_variable")
                .then(|| variable_field(reporter, "VARIABLE", variables))?
        }
        _ => None,
    }
}

pub(super) fn owned_input(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    arguments: &HashMap<String, String>,
) -> String {
    if let Some(source) = variable_input(target, input, variables, lists, arguments) {
        if source.starts_with("script.numeric_argument(")
            || source.starts_with("__aida_numeric_arg_")
        {
            format!("ScratchValue::Number({source})")
        } else {
            format!("({source}).clone()")
        }
    } else {
        let value = expression(
            target,
            input,
            variables,
            lists,
            arguments,
            &mut HashSet::new(),
        );
        format!("({value}).into()")
    }
}

// lower fixed pen dropdowns to enum values so repeated blocks avoid string matching.
pub(super) fn pen_color_parameter_input(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    arguments: &HashMap<String, String>,
) -> String {
    let static_value = match input.and_then(parse_input) {
        None | Some(InputValue::Empty) => Some("color"),
        Some(InputValue::String(value)) => value.as_str(),
        Some(InputValue::BlockReference(id)) => match target.blocks.get(id) {
            Some(block) if block.opcode == "pen_menu_colorParam" => {
                field_value(block, "colorParam").and_then(Value::as_str)
            }
            _ => None,
        },
        Some(InputValue::Number(_))
        | Some(InputValue::VariableReference(_))
        | Some(InputValue::ListReference(_)) => None,
    };

    if let Some(value) = static_value {
        let variant = match value {
            "color" => "Color",
            "saturation" => "Saturation",
            "brightness" => "Brightness",
            "transparency" => "Transparency",
            _ => "Unsupported",
        };
        return format!("crate::runtime::PenColorParameter::{variant}");
    }

    let value = expression(
        target,
        input,
        variables,
        lists,
        arguments,
        &mut HashSet::new(),
    );
    format!("scratch_string({value})")
}

// preserve hsv state whenever any block on this target can read or change it.
pub(super) fn target_has_pen_hsv_controls(target: &ScratchTarget) -> bool {
    target.blocks.values().any(|block| {
        matches!(
            block.opcode.as_str(),
            "pen_changePenColorParamBy"
                | "pen_setPenColorParamTo"
                | "pen_changePenHueBy"
                | "pen_setPenHueToNumber"
                | "pen_changePenShadeBy"
                | "pen_setPenShadeToNumber"
        )
    })
}

pub(super) fn list_field(
    block: &ScratchBlock,
    field: &str,
    lists: &HashMap<String, ListInfo>,
) -> Option<ListInfo> {
    let data = block.fields.get(field)?.as_array()?;
    lists.get(data.get(1)?.as_str()?).cloned()
}

// resolve a block's variable field ID to its allocated Rust identifier.
pub(in crate::rust) fn variable_field(
    block: &ScratchBlock,
    field: &str,
    variables: &HashMap<String, VariableInfo>,
) -> Option<VariableInfo> {
    let data = block.fields.get(field)?.as_array()?;
    super::super::project::variable_for_target_index(
        block.target_index,
        data.get(1)?.as_str()?,
        variables,
    )
    .cloned()
}

// get the original Scratch variable name displayed by a say reporter.
pub(super) fn say_variable_name(target: &ScratchTarget, input: Option<&Value>) -> Option<String> {
    match input.and_then(parse_input)? {
        InputValue::VariableReference(reference) => Some(reference.name.to_owned()),
        InputValue::BlockReference(id) => {
            let reporter = target.blocks.get(id)?;
            if reporter.opcode != "data_variable" {
                return None;
            }
            field_value(reporter, "VARIABLE")?
                .as_str()
                .map(str::to_owned)
        }
        _ => None,
    }
}
