//! Pretty-print a PlayMaker FSM: the state graph plus, per state, every action's class and parameters.
use std::fmt::Write;
use std::io::IsTerminal;
use std::sync::LazyLock;

use anyhow::Result;
use playmakerfsm::model::{
    Action, FsmModel, ParamValue, State, TemplateControl, Transition, decode_fsm, longest_ascii_run,
};
use playmakerfsm::raw::*;
use rabex::objects::pptr::PathId;
use rabex_env::rabex::objects::PPtr;

mod utils;

fn main() -> Result<()> {
    let env = utils::find_game("silksong")?.unwrap();

    let bundle = "scenes_scenes_scenes/tut_04.bundle";
    let path_id: PathId = 4720;

    let file = env.load_addressables_bundle_content(bundle)?;
    let fsm = file.object_at::<PlayMakerFSM>(path_id)?.read()?;
    let fsm = decode_fsm(&fsm.fsm);

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
        m.event_count
    );

    let total_actions: usize = m.states.iter().map(|s| s.actions.len()).sum();
    if m.states.len() <= 1 && total_actions == 0 && m.event_count == 0 {
        let _ = writeln!(
            o,
            "  ⚠ STUB: empty graph, logic lives in C# — variable container only."
        );
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
    // array elements follow their `Array` entry flat in the param list; indent them under it.
    let mut array_remaining = 0usize;
    for p in &a.params {
        let s = fmt_value(&p.value, p.type_name);
        let colored = match &p.value {
            ParamValue::Event(Some(_)) => event(&s),
            _ if s.starts_with("var ") => var(&s),
            _ => s,
        };
        let name = if p.name.is_empty() { "·" } else { p.name };
        let indent = if array_remaining > 0 {
            array_remaining -= 1;
            "              "
        } else {
            "          "
        };
        let _ = writeln!(
            o,
            "{}{} {} {}",
            indent,
            name,
            dim(&format!(": {} =", p.type_name)),
            colored
        );
        if let ParamValue::ArraySize(n) = &p.value {
            array_remaining = *n as usize;
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
        ParamValue::Owner(ow) => fmt_owner(ow),
        ParamValue::Var(fv) => fmt_var(fv),
        ParamValue::GameObject(g) => fmt_go(g.useVariable, &g.name, &g.value),
        ParamValue::Object(g) => fmt_go(g.useVariable, &g.name, &g.value),
        ParamValue::EventTarget(t) => fmt_event_target(t),
        ParamValue::Function(f) => fmt_function(f),
        ParamValue::Template(t) => fmt_template(t),
        ParamValue::Enum(e) => fmt_enum(e),
        ParamValue::Array(a) => fmt_array(a),
        ParamValue::Property(p) => fmt_property(p),
        ParamValue::AnimCurve(c) => format!("curve[{} keys]", c.curve.m_Curve.len()),
        ParamValue::ArraySize(n) => format!("[{n} elems]"),
        ParamValue::Pptr(p) => fmt_pptr(p),
        ParamValue::Raw(bytes) => match longest_ascii_run(bytes) {
            Some(s) => format!("→{s:?}"),
            None => format!("({type_name}, {}B)", bytes.len()),
        },
    }
}

fn fmt_string(s: &FsmString) -> String {
    if s.useVariable != 0 && !s.name.is_empty() {
        format!("var {:?}", s.name)
    } else {
        format!("{:?}", s.value)
    }
}
fn fmt_go(use_var: u8, name: &str, pptr: &PPtr) -> String {
    if use_var != 0 && !name.is_empty() {
        format!("var {name:?}")
    } else {
        fmt_pptr(pptr)
    }
}
fn fmt_pptr(p: &PPtr) -> String {
    if p.is_null() {
        "<null>".into()
    } else {
        format!("PPtr({})", p.m_PathID)
    }
}
fn fmt_owner(o: &FsmOwnerDefault) -> String {
    if o.ownerOption == 0 {
        "Owner (Self)".into()
    } else {
        fmt_go(
            o.gameObject.useVariable,
            &o.gameObject.name,
            &o.gameObject.value,
        )
    }
}
fn fmt_var(v: &FsmVar) -> String {
    if !v.variableName.is_empty() {
        return format!("var {:?}", v.variableName);
    }
    // a variable slot (e.g. a storeResult) left unbound — not an inline constant of 0/"".
    if v.useVariable != 0 {
        return "(unset var)".into();
    }
    // inline constant — VariableType: 0 Float 1 Int 2 Bool 3 GameObject 4 String … 14 Enum, -1 unused.
    match v.r#type {
        -1 => "(unused)".into(),
        0 => format!("{}", v.floatValue),
        1 => v.intValue.to_string(),
        2 => (v.boolValue != 0).to_string(),
        4 => format!("{:?}", v.stringValue),
        14 => format!("enum({})", v.intValue),
        3 | 9 | 10 | 12 => fmt_pptr(&v.objectReference),
        5 | 6 | 7 | 8 | 11 => {
            let w = &v.vector4Value;
            format!("({},{},{},{})", w.x, w.y, w.z, w.w)
        }
        _ => "<inline>".into(),
    }
}
fn fmt_event_target(t: &FsmEventTarget) -> String {
    let kind = match t.target {
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
    if t.target == 1 || t.target == 2 {
        bits.push(if t.gameObject.ownerOption == 0 {
            "Owner".to_string()
        } else if !t.gameObject.gameObject.name.is_empty() {
            format!("var {:?}", t.gameObject.gameObject.name)
        } else {
            "<unset>".to_string()
        });
    }
    if !t.fsmName.value.is_empty() {
        bits.push(format!("fsm={:?}", t.fsmName.value));
    }
    if bits.is_empty() {
        kind.to_string()
    } else {
        format!("{}({})", kind, bits.join(", "))
    }
}
fn fmt_function(f: &FunctionCall) -> String {
    if f.parameterType.is_empty() || f.parameterType == "None" {
        format!("{}()", f.FunctionName)
    } else {
        format!("{}(<{}>)", f.FunctionName, f.parameterType)
    }
}
fn fmt_template(t: &TemplateControl) -> String {
    // each override binds a template variable to a parent-FSM variable; `arrow` shows the data flow
    // (inputs: parent -> template, rendered `templateVar <- parentVar`; outputs: template -> parent, `->`).
    let vmap = |entries: &[&FsmVarOverride], arrow: &str| -> Vec<String> {
        entries
            .iter()
            .map(|e| {
                if !e.fsmVar.variableName.is_empty() {
                    format!("{}{arrow}var {:?}", e.variable.name, e.fsmVar.variableName)
                } else {
                    e.variable.name.clone()
                }
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
fn fmt_enum(e: &FsmEnum) -> String {
    if e.useVariable != 0 && !e.name.is_empty() {
        format!("var {:?}", e.name)
    } else if !e.enumName.is_empty() {
        format!("{}({})", short(&e.enumName), e.intValue)
    } else {
        e.intValue.to_string()
    }
}
fn fmt_array(a: &FsmArray) -> String {
    if a.useVariable != 0 && !a.name.is_empty() {
        return format!("var {:?}", a.name);
    }
    let n = a.floatValues.len()
        + a.intValues.len()
        + a.boolValues.len()
        + a.stringValues.len()
        + a.vector4Values.len()
        + a.objectReferences.len();
    format!("array[{n} elems]")
}
fn fmt_property(p: &FsmProperty) -> String {
    let ty = short(&p.TargetTypeName);
    if p.PropertyName.is_empty() {
        ty.to_string()
    } else {
        format!("{}.{}", ty, p.PropertyName)
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
