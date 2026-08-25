//! Render an [`FsmModel`] as pseudocode.
//!
//! PlayMaker stores a visual state machine in a shape nobody can read: every
//! action's parameters are flattened into parallel `paramName` /
//! `paramDataType` / `paramDataPos` arrays plus a `byteData` blob, sliced per
//! action by `actionStartIndex`. [`crate::model`] decodes that back into a
//! typed model; this module turns the model into a state/transition/action
//! listing, with the variables last in two paragraphs — the ones the PlayMaker
//! inspector exposes first:
//!
//! ```text
//! // uses template: bell_shrine
//! fsm Bell Shrine {
//!   start Init
//!   on RESET → Init  // from any state
//!
//!   state Init {
//!     SetBoolValue(boolVariable=var "Activated", boolValue=true)
//!     on FINISHED → Idle
//!   }
//!
//!   var Activated: bool = false
//! }
//! ```

use std::fmt::Write as _;

use crate::model::{
    Action, ArrayValue, Call, Curve, EnumValue, EventTarget, FsmModel, GoRef, ObjectRef, Param,
    ParamValue, Property, RefTarget, StrValue, TemplateControl, Transition, Value, VarOverride,
    VarValue, Variable,
};

/// Render the whole FSM. Ends with a newline.
pub fn render(model: &FsmModel<'_>) -> String {
    let mut out = String::new();
    let mut line = |indent: usize, text: &str| {
        for _ in 0..indent {
            out.push_str("  ");
        }
        out.push_str(text);
        out.push('\n');
    };

    if let Some(template) = &model.template_name {
        line(0, &format!("// uses template: {template}"));
    }
    line(0, &format!("fsm {} {{", model.name));
    line(1, &format!("start {}", model.start_state));

    for transition in &model.global_transitions {
        line(
            1,
            &format!("{}  // from any state", transition_text(transition)),
        );
    }

    for state in &model.states {
        line(0, "");
        line(1, &format!("state {} {{", state.name));
        for action in &state.actions {
            line(2, &action_text(action));
        }
        for transition in &state.transitions {
            line(2, &transition_text(transition));
        }
        line(1, "}");
    }

    // Inspector-exposed first, in their own paragraph: those are the ones an
    // instance of a template can set, so they are where two instances differ.
    for exposed in [true, false] {
        let group = model
            .variables
            .iter()
            .filter(|v| v.show_in_inspector == exposed);
        let mut any = false;
        for variable in group {
            if !any {
                line(0, "");
                any = true;
            }
            line(1, &var_decl(variable));
        }
    }

    line(0, "}");
    out
}

fn var_decl(variable: &Variable<'_>) -> String {
    format!(
        "var {}: {} = {}",
        variable.name,
        variable.category,
        value(&variable.value)
    )
}

fn transition_text(transition: &Transition<'_>) -> String {
    format!("on {} → {}", transition.event, transition.to_state)
}

fn action_text(action: &Action<'_>) -> String {
    let mut text = format!("{}({})", short(&action.class), params(&action.params));
    let mut notes = Vec::new();
    if let Some(label) = &action.custom_name
        && !is_default_label(label, short(&action.class))
    {
        notes.push(label.as_ref());
    }
    if !action.enabled {
        notes.push("disabled");
    }
    if !notes.is_empty() {
        let _ = write!(text, "  // {}", notes.join(", "));
    }
    text
}

/// PlayMaker labels an action with its class name split at the capitals, so a
/// label that collapses back to the class name carries nothing a reader of the
/// class name doesn't already have.
fn is_default_label(label: &str, class: &str) -> bool {
    label
        .chars()
        .filter(|c| !c.is_whitespace())
        .eq(class.chars())
}

fn params(params: &[Param<'_>]) -> String {
    let mut out = String::new();
    for (i, param) in params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        if !param.name.is_empty() {
            let _ = write!(out, "{}=", param.name);
        }
        out.push_str(&param_value(&param.value));
    }
    out
}

fn param_value(value_: &ParamValue<'_>) -> String {
    match value_ {
        ParamValue::Bool(b) => b.to_string(),
        ParamValue::Int(i) => i.to_string(),
        ParamValue::Float(f) => num(*f),
        ParamValue::Vector(components) => vector(components),
        ParamValue::PackedVar(None) => "(unset)".to_string(),
        ParamValue::PackedVar(Some(name)) => format!("var {}", q(name)),
        ParamValue::Event(None) => "(none)".to_string(),
        ParamValue::Event(Some(name)) => format!("→{}", q(name)),
        ParamValue::Str(s) => q(s),
        ParamValue::FsmString(s) => str_value(s),
        ParamValue::Owner(r) | ParamValue::GameObject(r) | ParamValue::Object(r) => go_ref(r),
        ParamValue::Var(v) => var_value(v),
        ParamValue::EventTarget(t) => event_target(t),
        ParamValue::Function(call) => function_call(call),
        ParamValue::Template(t) => template_control(t),
        ParamValue::Enum(e) => enum_value(e),
        ParamValue::EnumMember(name) => name.to_string(),
        ParamValue::Layer { index, name } => match name {
            Some(name) => format!("{name}({index})"),
            None => index.to_string(),
        },
        ParamValue::Array(a) => array_value(a),
        ParamValue::Property(p) => property(p),
        ParamValue::AnimCurve(c) => curve(c),
        ParamValue::List(items) => format!("[{}]", params(items)),
        ParamValue::Pptr(r) => object_ref(r),
        // Params the decoder couldn't make sense of keep their byte length, so
        // a changed blob still shows up as a change.
        ParamValue::Raw(bytes) => format!("({}B)", bytes.len()),
    }
}

fn value(value_: &Value) -> String {
    match value_ {
        Value::Var(name) => format!("var {}", q(name)),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => num(*f),
        Value::Str(s) => q(s),
        Value::Vector(components) => vector(components),
        Value::Enum { enum_name, value } => format!("{}({value})", short(enum_name)),
        Value::Object(r) => object_ref(r),
        Value::Array(a) => array_value(a),
    }
}

fn var_value(value_: &VarValue) -> String {
    match value_ {
        VarValue::Var(name) => format!("var {}", q(name)),
        VarValue::Unset => "(unset var)".to_string(),
        VarValue::Unused => "(unused)".to_string(),
        VarValue::Float(f) => num(*f),
        VarValue::Int(i) => i.to_string(),
        VarValue::Bool(b) => b.to_string(),
        VarValue::Str(s) => q(s),
        VarValue::Object(r) => object_ref(r),
        VarValue::Vector(components) => vector(components),
        VarValue::Enum(i) => format!("enum({i})"),
        VarValue::Array(a) => array_value(a),
    }
}

fn str_value(value_: &StrValue) -> String {
    match value_ {
        StrValue::Var(name) => format!("var {}", q(name)),
        StrValue::Literal(s) => q(s),
    }
}

fn enum_value(value_: &EnumValue) -> String {
    match value_ {
        EnumValue::Var(name) => format!("var {}", q(name)),
        EnumValue::Named { enum_name, value } => format!("{}({value})", short(enum_name)),
        EnumValue::Value(i) => i.to_string(),
    }
}

fn array_value(value_: &ArrayValue) -> String {
    match value_ {
        ArrayValue::Var(name) => format!("var {}", q(name)),
        ArrayValue::Values(values) => {
            let elements: Vec<String> = values.iter().map(value).collect();
            format!("[{}]", elements.join(", "))
        }
    }
}

fn go_ref(r: &GoRef) -> String {
    match r {
        GoRef::SelfOwner => "Self".to_string(),
        GoRef::Var(name) => format!("var {}", q(name)),
        GoRef::Object(o) => object_ref(o),
    }
}

fn object_ref(r: &ObjectRef) -> String {
    let target = match &r.target {
        RefTarget::Null => return "<null>".to_string(),
        RefTarget::Path(path) => path.to_string(),
        RefTarget::Loose { name: Some(n), .. } => n.clone(),
        RefTarget::Loose { name: None, id } => format!("loose:{id}"),
    };
    match &r.file {
        Some(file) => format!("{target} ({file})"),
        None => target,
    }
}

fn event_target(target: &EventTarget) -> String {
    let kind = match target.kind {
        0 => "Self",
        1 => "GameObject",
        2 => "GameObjectFSM",
        3 => "FSMComponent",
        4 => "BroadcastAll",
        5 => "HostFSM",
        6 => "SubFSMs",
        _ => "?",
    };
    let mut parts = Vec::new();
    // The other kinds ignore the gameobject field, so printing it would show a
    // stale authored value.
    if matches!(target.kind, 1 | 2) {
        parts.push(go_ref(&target.game_object));
    }
    if let Some(name) = &target.fsm_name {
        parts.push(format!("fsm={}", q(name)));
    }
    if parts.is_empty() {
        kind.to_string()
    } else {
        format!("{kind}({})", parts.join(", "))
    }
}

fn function_call(call: &Call<'_>) -> String {
    if call.parameter_type.is_empty() || call.parameter_type == "None" {
        return format!("{}()", call.function);
    }
    match &call.value {
        Some(value_) => format!("{}({})", call.function, value(value_)),
        // Declares a parameter the decoder couldn't read — keep the bare type
        // so the call still says what it expects.
        None => format!("{}(<{}>)", call.function, call.parameter_type),
    }
}

fn property(property: &Property<'_>) -> String {
    let member = if property.property.is_empty() {
        short(&property.type_name).to_string()
    } else {
        format!("{}.{}", short(&property.type_name), property.property)
    };
    format!("{member} on {}", go_ref(&property.target))
}

/// A curve as its `time:value` keys. Tangents and weights are left out: they
/// shape the interpolation but say nothing about what the action does.
fn curve(curve: &Curve) -> String {
    let keys: Vec<String> = curve
        .keys
        .iter()
        .map(|k| format!("{}:{}", num(k.time), num(k.value)))
        .collect();
    format!("curve[{}]", keys.join(", "))
}

fn template_control(control: &TemplateControl<'_>) -> String {
    let mut parts = vec![format!("template={}", control.template)];
    for (label, arrow, bindings) in [
        ("in", "<-", &control.inputs),
        ("out", "->", &control.outputs),
        ("vars", "=", &control.overrides),
    ] {
        if bindings.is_empty() {
            continue;
        }
        let rendered: Vec<String> = bindings
            .iter()
            .map(|binding| var_override(binding, arrow))
            .collect();
        parts.push(format!("{label}[{}]", rendered.join(", ")));
    }
    if !control.events.is_empty() {
        let events: Vec<String> = control
            .events
            .iter()
            .map(|(from, to)| format!("{from}->{to}"))
            .collect();
        parts.push(format!("events[{}]", events.join(", ")));
    }
    parts.join(" ")
}

fn var_override(binding: &VarOverride, arrow: &str) -> String {
    format!("{}{arrow}{}", binding.variable, var_value(&binding.value))
}

/// Last segment of a dotted name — action and enum types are namespaced
/// (`HutongGames.PlayMaker.Actions.SetBoolValue`) and the namespace is the
/// same for all of them.
fn short(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// JSON-quoted, so embedded quotes and newlines stay on one line.
fn q(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("{s:?}"))
}

fn num(f: f32) -> String {
    format!("{f}")
}

fn vector(components: &[f32]) -> String {
    let parts: Vec<String> = components.iter().copied().map(num).collect();
    format!("({})", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use crate::model::{Curve, CurveKey, State, StatePos};

    use super::*;

    fn param<'a>(name: &'a str, value: ParamValue<'a>) -> Param<'a> {
        Param {
            name: name.into(),
            type_name: "FsmVar".into(),
            value,
        }
    }

    /// The expected text is the format's specification.
    #[test]
    fn renders_the_documented_layout() {
        let model = FsmModel {
            name: "Bell Shrine".into(),
            template_name: Some("bell_shrine".into()),
            start_state: "Init".into(),
            events: Vec::new(),
            global_transitions: vec![Transition {
                event: "RESET".into(),
                to_state: "Init".into(),
            }],
            variables: vec![
                Variable {
                    name: "On".into(),
                    category: "bool".into(),
                    show_in_inspector: true,
                    value: Value::Bool(false),
                },
                Variable {
                    name: "Ticks".into(),
                    category: "int".into(),
                    show_in_inspector: false,
                    value: Value::Int(3),
                },
            ],
            states: vec![
                State {
                    name: "Init".into(),
                    is_start: true,
                    color_index: 0,
                    position: StatePos {
                        x: 0.0,
                        y: 0.0,
                        w: 0.0,
                        h: 0.0,
                    },
                    transitions: vec![Transition {
                        event: "FINISHED".into(),
                        to_state: "Idle".into(),
                    }],
                    actions: vec![Action {
                        class: "HutongGames.PlayMaker.Actions.SetBoolValue".into(),
                        custom_name: Some("arm the bell".into()),
                        enabled: true,
                        params: vec![
                            param(
                                "boolVariable",
                                ParamValue::PackedVar(Some("On".to_string())),
                            ),
                            param("boolValue", ParamValue::Bool(true)),
                            param("finishEvent", ParamValue::Event(None)),
                            param("target", ParamValue::Owner(GoRef::SelfOwner)),
                            param("colour", ParamValue::Vector(vec![1.0, 0.5, 0.0])),
                            param("offset", ParamValue::Var(VarValue::Vector(vec![1.0, 2.0]))),
                            param(
                                "property",
                                ParamValue::Property(Property {
                                    target: GoRef::Var("Target".to_string()),
                                    type_name: "UnityEngine.Transform".into(),
                                    property: "position".into(),
                                    set: true,
                                }),
                            ),
                            param(
                                "curve",
                                ParamValue::AnimCurve(Curve {
                                    keys: vec![CurveKey {
                                        time: 0.0,
                                        value: 1.0,
                                        in_slope: 0.0,
                                        out_slope: 0.0,
                                        in_weight: 0.0,
                                        out_weight: 0.0,
                                        weighted_mode: 0,
                                    }],
                                    pre_infinity: 0,
                                    post_infinity: 0,
                                    rotation_order: 0,
                                }),
                            ),
                            param(
                                "parameters",
                                ParamValue::List(vec![param("", ParamValue::Int(3))]),
                            ),
                            param(
                                "sendTo",
                                ParamValue::EventTarget(EventTarget {
                                    kind: 2,
                                    game_object: GoRef::Var("Bell".to_string()),
                                    fsm_name: Some("Control".to_string()),
                                    fsm: ObjectRef {
                                        file: None,
                                        target: RefTarget::Null,
                                    },
                                    exclude_self: false,
                                    send_to_children: false,
                                }),
                            ),
                        ],
                    }],
                },
                State {
                    name: "Idle".into(),
                    is_start: false,
                    color_index: 0,
                    position: StatePos {
                        x: 0.0,
                        y: 0.0,
                        w: 0.0,
                        h: 0.0,
                    },
                    transitions: Vec::new(),
                    actions: vec![
                        Action {
                            class: "HutongGames.PlayMaker.Actions.SendEvent".into(),
                            custom_name: None,
                            enabled: false,
                            params: vec![param("value", ParamValue::Raw(vec![1, 2, 3].into()))],
                        },
                        Action {
                            class: "HutongGames.PlayMaker.Actions.SetFsmBool".into(),
                            // PlayMaker's own label for the class, so it is dropped
                            custom_name: Some("Set Fsm Bool".into()),
                            enabled: true,
                            params: Vec::new(),
                        },
                    ],
                },
            ],
        };

        let expected = concat!(
            "// uses template: bell_shrine\n",
            "fsm Bell Shrine {\n",
            "  start Init\n",
            "  on RESET → Init  // from any state\n",
            "\n",
            "  state Init {\n",
            "    SetBoolValue(boolVariable=var \"On\", boolValue=true, finishEvent=(none), ",
            "target=Self, colour=(1, 0.5, 0), offset=(1, 2), ",
            "property=Transform.position on var \"Target\", curve=curve[0:1], parameters=[3], ",
            "sendTo=GameObjectFSM(var \"Bell\", fsm=\"Control\"))  // arm the bell\n",
            "    on FINISHED → Idle\n",
            "  }\n",
            "\n",
            "  state Idle {\n",
            "    SendEvent(value=(3B))  // disabled\n",
            "    SetFsmBool()\n",
            "  }\n",
            "\n",
            "  var On: bool = false\n",
            "\n",
            "  var Ticks: int = 3\n",
            "}\n",
        );
        assert_eq!(render(&model), expected);
    }
}
