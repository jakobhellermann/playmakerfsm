//! Pretty-print a PlayMaker FSM: the state graph plus, per state, every action's class and parameters.
use std::fmt::Write;
use std::io::IsTerminal;
use std::sync::LazyLock;

use anyhow::Result;
use playmakerfsm::model::{
    Action, ArrayValue, Call, Context, EnumValue, EventTarget, FsmModel, GoRef, ObjectRef,
    ParamValue, Property, RefTarget, State, StrValue, TemplateControl, Transition, VarOverride,
    VarValue, decode_fsm, longest_ascii_run,
};
use playmakerfsm::raw::*;
use rabex::objects::pptr::PathId;

mod utils;

fn main() -> Result<()> {
    let env = utils::find_game("silksong")?.unwrap();

    let bundle = "scenes_scenes_scenes/tut_04.bundle";
    let path_id: PathId = 4720;

    let file = env.load_addressables_bundle_content(bundle)?;
    let fsm = file.object_at::<PlayMakerFSM>(path_id)?.read()?;
    let mut ctx = Context::new(&file);
    let fsm = decode_fsm(&fsm.fsm, &mut ctx);

    print!("{}", prettify_model(&fsm));
    Ok(())
}

pub fn prettify_model(m: &FsmModel) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "FSM {:?}  start={:?}  states={}  events={}",
        m.name,
        m.start_state,
        m.states.len(),
        m.events.len()
    );

    let total_actions: usize = m.states.iter().map(|s| s.actions.len()).sum();
    if m.states.len() <= 1 && total_actions == 0 && m.events.is_empty() {
        let _ = writeln!(
            o,
            "  ⚠ STUB: empty graph, logic lives in C# — variable container only."
        );
    }

    if !m.events.is_empty() {
        let _ = writeln!(o, "\nEVENTS:");
        for e in &m.events {
            let flags = match (e.is_global, e.is_system) {
                (true, true) => "  (global, system)",
                (true, false) => "  (global)",
                (false, true) => "  (system)",
                (false, false) => "",
            };
            let _ = writeln!(o, "  {}{}", event(e.name), dim(flags));
        }
    }

    if !m.global_transitions.is_empty() {
        let _ = writeln!(o, "\nGLOBAL TRANSITIONS (from any state):");
        for t in &m.global_transitions {
            write_transition(&mut o, t);
        }
    }

    let _ = writeln!(o, "\nSTATES:");
    for s in &m.states {
        write_state(&mut o, s);
    }

    if !m.variables.is_empty() {
        let _ = writeln!(o, "\nVARIABLES:");
        for v in &m.variables {
            let _ = writeln!(o, "  {} {}", dim(&format!("({})", v.category)), var(v.name));
        }
    }
    o
}

fn write_transition(o: &mut String, t: &Transition) {
    let _ = writeln!(
        o,
        "  on {} -> {}",
        event(&q(t.event)),
        state(&q(t.to_state))
    );
}

fn write_state(o: &mut String, s: &State) {
    let mark = if s.is_start { "*" } else { " " };
    let _ = writeln!(o, "\n {}[{}]", mark, state(s.name));
    for t in &s.transitions {
        let _ = writeln!(
            o,
            "      on {} -> {}",
            event(&q(t.event)),
            state(&q(t.to_state))
        );
    }
    for a in &s.actions {
        write_action(o, a);
    }
}

fn write_action(o: &mut String, a: &Action) {
    let dis = if a.enabled { "" } else { "  (DISABLED)" };
    // a user-given label that differs from the class name is worth surfacing.
    let custom = a
        .custom_name
        .filter(|c| *c != short(a.class))
        .map(|c| format!("  {}", dim(&format!("\"{c}\""))))
        .unwrap_or_default();
    let _ = writeln!(o, "      · {}{}{}", action(short(a.class)), custom, dis);
    for p in &a.params {
        write_param(o, p, 1);
    }
}

fn write_param(o: &mut String, p: &playmakerfsm::model::Param, depth: usize) {
    let s = fmt_value(&p.value, p.type_name);
    let colored = match &p.value {
        ParamValue::Event(Some(_)) => event(&s),
        _ if s.starts_with("var ") => var(&s),
        _ => s,
    };
    let name = if p.name.is_empty() { "·" } else { p.name };
    let indent = "    ".repeat(depth + 1);
    let _ = writeln!(
        o,
        "{}{} {} {}",
        indent,
        name,
        dim(&format!(": {} =", p.type_name)),
        colored
    );
    if let ParamValue::List(elems) = &p.value {
        for e in elems {
            write_param(o, e, depth + 1);
        }
    }
}

fn fmt_value(v: &ParamValue, type_name: &str) -> String {
    match v {
        ParamValue::Bool(b) => b.to_string(),
        ParamValue::Int(i) => i.to_string(),
        ParamValue::Float(f) => format!("{f}"),
        ParamValue::Vector(comps) => {
            let parts: Vec<_> = comps.iter().map(|f| format!("{f}")).collect();
            format!("({})", parts.join(", "))
        }
        ParamValue::PackedVar(Some(name)) => format!("var {name:?}"),
        ParamValue::PackedVar(None) => "<var>".into(),
        ParamValue::Event(Some(name)) => format!("→{name:?}"),
        ParamValue::Event(None) => "(none)".into(),
        ParamValue::Str(s) => format!("{s:?}"),
        ParamValue::FsmString(s) => fmt_string(s),
        ParamValue::Owner(r) => fmt_go_ref(r),
        ParamValue::Var(v) => fmt_var(v),
        ParamValue::GameObject(r) => fmt_go_ref(r),
        ParamValue::Object(r) => fmt_go_ref(r),
        ParamValue::EventTarget(t) => fmt_event_target(t),
        ParamValue::Function(f) => fmt_function(f),
        ParamValue::Template(t) => fmt_template(t),
        ParamValue::Enum(e) => fmt_enum(e),
        ParamValue::Array(a) => fmt_array(a),
        ParamValue::Property(p) => fmt_property(p),
        ParamValue::AnimCurve(c) => format!("curve[{} keys]", c.keys.len()),
        ParamValue::List(elems) => format!("[{} elems]", elems.len()),
        ParamValue::Pptr(r) => fmt_object_ref(r),
        ParamValue::Raw(bytes) => match longest_ascii_run(bytes) {
            Some(s) => format!("→{s:?}"),
            None => format!("({type_name}, {}B)", bytes.len()),
        },
    }
}

fn fmt_string(s: &StrValue) -> String {
    match s {
        StrValue::Var(name) => format!("var {name:?}"),
        StrValue::Literal(value) => format!("{value:?}"),
    }
}
fn fmt_go_ref(r: &GoRef) -> String {
    match r {
        GoRef::SelfOwner => "Owner (Self)".into(),
        GoRef::Var(name) => format!("var {name:?}"),
        GoRef::Object(o) => fmt_object_ref(o),
    }
}
fn fmt_object_ref(r: &ObjectRef) -> String {
    let loc = match &r.target {
        RefTarget::Path(p) => p.to_string(),
        RefTarget::Loose { name: Some(n), .. } => n.clone(),
        RefTarget::Loose { name: None, id } => format!("loose:{id}"),
        RefTarget::Null => return "<null>".into(),
    };
    match &r.file {
        Some(file) => format!("{loc} ({file})"),
        None => loc,
    }
}
fn fmt_var(v: &VarValue) -> String {
    match v {
        VarValue::Var(name) => format!("var {name:?}"),
        VarValue::Unset => "(unset var)".into(),
        VarValue::Unused => "(unused)".into(),
        VarValue::Float(f) => format!("{f}"),
        VarValue::Int(i) => i.to_string(),
        VarValue::Bool(b) => b.to_string(),
        VarValue::Str(s) => format!("{s:?}"),
        VarValue::Object(o) => fmt_object_ref(o),
        VarValue::Vector(comps) => {
            let parts: Vec<_> = comps.iter().map(|f| f.to_string()).collect();
            format!("({})", parts.join(","))
        }
        VarValue::Enum(i) => format!("enum({i})"),
        VarValue::Array(a) => fmt_array(a),
    }
}
fn fmt_event_target(t: &EventTarget) -> String {
    let kind = match t.kind {
        0 => "Self",
        1 => "GameObject",
        2 => "GameObjectFSM",
        3 => "FSMComponent",
        4 => "BroadcastAll",
        5 => "HostFSM",
        6 => "SubFSMs",
        _ => "?",
    };
    let mut bits = Vec::new();
    if t.kind == 1 || t.kind == 2 {
        bits.push(fmt_go_ref(&t.game_object));
    }
    if let Some(name) = &t.fsm_name {
        bits.push(format!("fsm={name:?}"));
    }
    if bits.is_empty() {
        kind.to_string()
    } else {
        format!("{}({})", kind, bits.join(", "))
    }
}
fn fmt_function(f: &Call) -> String {
    if f.parameter_type.is_empty() || f.parameter_type == "None" {
        format!("{}()", f.function)
    } else {
        format!("{}(<{}>)", f.function, f.parameter_type)
    }
}
fn fmt_template(t: &TemplateControl) -> String {
    // each override binds a template variable to a parent-FSM variable; `arrow` shows the data flow
    // (inputs: parent -> template, rendered `templateVar <- parentVar`; outputs: template -> parent, `->`).
    let vmap = |entries: &[VarOverride], arrow: &str| -> Vec<String> {
        entries
            .iter()
            .map(|o| match &o.value {
                VarValue::Var(name) => format!("{}{arrow}var {name:?}", o.variable),
                _ => o.variable.clone(),
            })
            .collect()
    };
    let mut parts = vec![format!("template={}", t.template)];
    for (label, vars) in [
        ("in", vmap(&t.inputs, "<-")),
        ("out", vmap(&t.outputs, "->")),
        ("vars", vmap(&t.overrides, "=")),
    ] {
        if !vars.is_empty() {
            parts.push(format!("{label}[{}]", vars.join(", ")));
        }
    }
    if !t.events.is_empty() {
        let evs: Vec<_> = t
            .events
            .iter()
            .map(|(f, to)| format!("{f}->{to}"))
            .collect();
        parts.push(format!("events[{}]", evs.join(", ")));
    }
    parts.join(" ")
}
fn fmt_enum(e: &EnumValue) -> String {
    match e {
        EnumValue::Var(name) => format!("var {name:?}"),
        EnumValue::Named { enum_name, value } => format!("{}({value})", short(enum_name)),
        EnumValue::Value(value) => value.to_string(),
    }
}
fn fmt_array(a: &ArrayValue) -> String {
    match a {
        ArrayValue::Var(name) => format!("var {name:?}"),
        ArrayValue::Values(values) => format!("array[{} elems]", values.len()),
    }
}
fn fmt_property(p: &Property) -> String {
    let ty = short(p.type_name);
    if p.property.is_empty() {
        ty.to_string()
    } else {
        format!("{}.{}", ty, p.property)
    }
}

// ── colors (auto-off when piped / NO_COLOR): event=yellow state=cyan action=green variable=magenta ──
static COLOR: LazyLock<bool> =
    LazyLock::new(|| std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none());

fn paint(code: &str, s: &str) -> String {
    if *COLOR {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}
fn event(s: &str) -> String {
    paint("33", s)
}
fn state(s: &str) -> String {
    paint("36", s)
}
fn action(s: &str) -> String {
    paint("32", s)
}
fn var(s: &str) -> String {
    paint("35", s)
}
fn dim(s: &str) -> String {
    paint("2", s)
}

fn q(s: &str) -> String {
    format!("{s:?}")
}
fn short(cls: &str) -> &str {
    cls.rsplit('.').next().unwrap_or(cls)
}
