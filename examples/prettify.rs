//! Pretty-print a PlayMaker FSM: the state graph plus, per state, every action's class and its
//! parameters decoded from the `ActionData` parallel-array encoding.
//!
//! ActionData stores all params of a state flat across `paramName/paramDataType/paramDataPos`, sliced
//! per action by `actionStartIndex` (no trailing sentinel — the last action runs to the end). Reference
//! params (FsmString, FsmOwnerDefault, FsmVar, …) live in a typed array that `paramDataPos` indexes;
//! value primitives (Boolean/Integer/Float and the packed Fsm-wrappers/FsmEvent) live in the flat
//! `byteData` at byte-offset `paramDataPos` (`paramByteDataSize` = length). The Fsm scalar wrappers pack
//! as `[value(valsize)][useVariable(1)][name(rest)]`.
//!
//! Rust port of `HornetPlayer/tools/prettify-fsm` (which decodes rabex `… object … cat` JSON); here we
//! work straight off the deserialized `Fsm` types.

use std::fmt::Write;
use std::io::IsTerminal;
use std::sync::LazyLock;

use anyhow::Result;
use playmakerfsm::raw::*;
use rabex::objects::pptr::PathId;
use rabex::{tpk::TpkTypeTreeBlob, typetree::typetree_cache::sync::TypeTreeCache};
use rabex_env::Environment;
use rabex_env::rabex::objects::PPtr;

fn main() -> Result<()> {
    let path = "/home/jakob/.steamapps/Hollow Knight Silksong";
    let tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
    let env = Environment::new_in(path, tpk)?;

    let bundle = "scenes_scenes_scenes/tut_04.bundle";
    let path_id: PathId = 4720;

    let file = env.load_addressables_bundle_content(bundle)?;
    let fsm = file.object_at::<PlayMakerFSM>(path_id)?.read()?;

    print!("{}", prettify_fsm(&fsm.fsm));
    Ok(())
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

pub fn prettify_fsm(fsm: &Fsm) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "FSM {:?}  start={:?}  states={}  events={}",
        fsm.name,
        fsm.startState,
        fsm.states.len(),
        fsm.events.len()
    );

    let total_actions: usize = fsm
        .states
        .iter()
        .map(|s| s.actionData.actionNames.len())
        .sum();
    if fsm.states.len() <= 1 && total_actions == 0 && fsm.events.is_empty() {
        let _ = writeln!(
            o,
            "  ⚠ STUB: empty graph, logic lives in C# — variable container only."
        );
    }

    if !fsm.globalTransitions.is_empty() {
        let _ = writeln!(o, "\nGLOBAL TRANSITIONS (from any state):");
        for t in &fsm.globalTransitions {
            let _ = writeln!(
                o,
                "  on {} -> {}",
                event(&q(&t.fsmEvent.name)),
                state(&q(&t.toState))
            );
        }
    }

    let _ = writeln!(o, "\nSTATES:");
    for s in &fsm.states {
        let mark = if s.name == fsm.startState { "*" } else { " " };
        let _ = writeln!(o, "\n {}[{}]", mark, state(&s.name));
        for t in &s.transitions {
            let _ = writeln!(
                o,
                "      on {} -> {}",
                event(&q(&t.fsmEvent.name)),
                state(&q(&t.toState))
            );
        }
        let ad = &s.actionData;
        for (ai, cls) in ad.actionNames.iter().enumerate() {
            let dis = if ad.actionEnabled.get(ai) == Some(&0) {
                "  (DISABLED)"
            } else {
                ""
            };
            // a user-given label (customNames) that differs from the class name is worth surfacing.
            let custom = ad
                .customNames
                .get(ai)
                .filter(|c| !c.is_empty() && c.as_str() != short(cls))
                .map(|c| format!("  {}", dim(&format!("\"{c}\""))))
                .unwrap_or_default();
            let _ = writeln!(o, "      · {}{}{}", action(short(cls)), custom, dis);
            for (name, tname, value) in decode_action(ad, ai) {
                let val = if tname == "FsmEvent" && value != "(none)" {
                    event(&value)
                } else if value.starts_with("var ") {
                    var(&value)
                } else {
                    value
                };
                let _ = writeln!(
                    o,
                    "          {} {} {}",
                    name,
                    dim(&format!(": {tname} =")),
                    val
                );
            }
        }
    }

    let named = named_vars(&fsm.variables);
    if !named.is_empty() {
        let _ = writeln!(o, "\nVARIABLES:");
        for (n, cat) in named {
            let _ = writeln!(o, "  {} {}", dim(&format!("({cat})")), var(&n));
        }
    }
    o
}

/// Decode action `ai`'s params -> (fieldName, typeName, renderedValue).
fn decode_action(ad: &ActionData, ai: usize) -> Vec<(String, &'static str, String)> {
    let starts = &ad.actionStartIndex;
    if ai >= starts.len() {
        return vec![];
    }
    let lo = starts[ai] as usize;
    // actionStartIndex has no end sentinel: the last action's params run to the end of the arrays.
    let hi = starts
        .get(ai + 1)
        .map(|&x| x as usize)
        .unwrap_or(ad.paramName.len());

    (lo..hi)
        .filter_map(|j| {
            let dt = *ad.paramDataType.get(j)?;
            let pos = *ad.paramDataPos.get(j)? as usize;
            let size = ad.paramByteDataSize.get(j).copied().unwrap_or(0) as usize;
            let tname = ptype(dt);
            let field = ad
                .paramName
                .get(j)
                .filter(|s| !s.is_empty())
                .map(String::as_str)
                .unwrap_or("·");
            Some((field.to_string(), tname, render_param(ad, tname, pos, size)))
        })
        .collect()
}

fn render_param(ad: &ActionData, tname: &str, pos: usize, size: usize) -> String {
    let bd = &ad.byteData;
    let opt = match tname {
        "FsmString" => ad.fsmStringParams.get(pos).map(fmt_string),
        "String" => ad.stringParams.get(pos).map(|s| format!("{s:?}")),
        "FsmOwnerDefault" => ad.fsmOwnerDefaultParams.get(pos).map(fmt_owner),
        "FsmVar" => ad.fsmVarParams.get(pos).map(fmt_var),
        "FsmGameObject" => ad
            .fsmGameObjectParams
            .get(pos)
            .map(|g| fmt_go(g.useVariable, &g.name, &g.value)),
        "FsmObject" | "FsmMaterial" | "FsmTexture" => ad
            .fsmObjectParams
            .get(pos)
            .map(|g| fmt_go(g.useVariable, &g.name, &g.value)),
        "FsmEventTarget" => ad.fsmEventTargetParams.get(pos).map(fmt_event_target),
        "FunctionCall" => ad.functionCallParams.get(pos).map(fmt_function),
        "FsmTemplateControl" => ad.fsmTemplateControlParams.get(pos).map(fmt_template),
        "Array" => ad.arrayParamSizes.get(pos).map(|n| format!("[{n} elems]")),
        "ObjectReference" | "GameObject" => ad.unityObjectParams.get(pos).map(fmt_pptr),
        "Boolean" => Some((bd.get(pos).copied().unwrap_or(0) != 0).to_string()),
        "Integer" | "Enum" | "LayerMask" => read_i32(bd, pos).map(|v| v.to_string()),
        "Float" => read_f32(bd, pos).map(|v| format!("{v}")),
        "FsmBool" => Some(fmt_packed(bd, pos, size, Packed::Bool)),
        "FsmInt" => Some(fmt_packed(bd, pos, size, Packed::Int)),
        "FsmFloat" => Some(fmt_packed(bd, pos, size, Packed::Float)),
        // value-type wrappers pack their floats into byteData (size == n*4 + useVariable byte [+ name]).
        "FsmVector2" => Some(fmt_packed_vec(bd, pos, size, 2)),
        "FsmVector3" => Some(fmt_packed_vec(bd, pos, size, 3)),
        "FsmQuaternion" | "FsmColor" | "FsmRect" => Some(fmt_packed_vec(bd, pos, size, 4)),
        // these carry no byteData (size 0): paramDataPos indexes the typed param array instead.
        "FsmEnum" => ad.fsmEnumParams.get(pos).map(fmt_enum),
        "FsmArray" => ad.fsmArrayParams.get(pos).map(fmt_array),
        "FsmProperty" => ad.fsmPropertyParams.get(pos).map(fmt_property),
        "FsmEvent" if size == 0 => Some("(none)".into()),
        _ => ascii_run(bd, pos, size).map(|s| format!("→{s:?}")),
    };
    opt.unwrap_or_else(|| format!("({tname}, {size}B)"))
}

// ── per-type formatters ──────────────────────────────────────────────────────────────────────────
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
fn fmt_template(t: &FsmTemplateControl) -> String {
    // each override binds a template variable to a parent-FSM variable; `arrow` shows the data flow
    // (inputs: parent -> template, rendered `templateVar <- parentVar`; outputs: template -> parent, `->`).
    let vmap = |entries: &[FsmVarOverride], arrow: &str| -> Vec<String> {
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
    let mut parts = vec![format!("template={}", t.target.m_PathID)];
    let ins = vmap(&t.inputVariables, "<-");
    if !ins.is_empty() {
        parts.push(format!("in[{}]", ins.join(", ")));
    }
    let outs = vmap(&t.outputVariables, "->");
    if !outs.is_empty() {
        parts.push(format!("out[{}]", outs.join(", ")));
    }
    let evs: Vec<_> = t
        .outputEvents
        .iter()
        .map(|e| format!("{}->{}", e.fromEvent.name, e.toEvent.name))
        .collect();
    if !evs.is_empty() {
        parts.push(format!("events[{}]", evs.join(", ")));
    }
    parts.join(" ")
}

// ── byteData decoders ────────────────────────────────────────────────────────────────────────────
fn read_i32(bd: &[u8], pos: usize) -> Option<i32> {
    bd.get(pos..pos + 4)
        .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
}
fn read_f32(bd: &[u8], pos: usize) -> Option<f32> {
    bd.get(pos..pos + 4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
}
#[derive(Clone, Copy)]
enum Packed {
    Bool,
    Int,
    Float,
}
/// Fsm scalar wrapper packed as [value(valsize)][useVariable(1)][name(rest)]. The wrapper type tells
/// us how to read the value bytes — int vs float can't be guessed (a packed `1` reads as a denormal).
fn fmt_packed(bd: &[u8], pos: usize, size: usize, kind: Packed) -> String {
    let valsize = match kind {
        Packed::Bool => 1,
        Packed::Int | Packed::Float => 4,
    };
    if let Some(v) = packed_var_name(bd, pos, size, valsize) {
        return v;
    }
    if size < valsize {
        return format!("({size}B packed)");
    }
    match kind {
        Packed::Bool => (bd.get(pos).copied().unwrap_or(0) != 0).to_string(),
        Packed::Int => read_i32(bd, pos).map(|i| i.to_string()).unwrap_or_default(),
        Packed::Float => read_f32(bd, pos)
            .map(|f| format!("{f}"))
            .unwrap_or_default(),
    }
}
/// Fsm value-type wrapper packing `n` floats as [f32 × n][useVariable(1)][name(rest)].
fn fmt_packed_vec(bd: &[u8], pos: usize, size: usize, n: usize) -> String {
    let valsize = n * 4;
    if let Some(v) = packed_var_name(bd, pos, size, valsize) {
        return v;
    }
    if size < valsize {
        return format!("({size}B packed)");
    }
    let comps: Vec<String> = (0..n)
        .map(|i| {
            read_f32(bd, pos + i * 4)
                .map(|f| format!("{f}"))
                .unwrap_or_default()
        })
        .collect();
    format!("({})", comps.join(", "))
}
/// Shared packed-wrapper variable decode: if the useVariable byte after the value is set, the rest is
/// the bound variable name. Returns None when the wrapper holds an inline value.
fn packed_var_name(bd: &[u8], pos: usize, size: usize, valsize: usize) -> Option<String> {
    if size > valsize && bd.get(pos + valsize).copied().unwrap_or(0) != 0 {
        let name = ascii_only(bd, pos + valsize + 1, pos + size);
        return Some(if name.is_empty() {
            "<var>".into()
        } else {
            format!("var {name:?}")
        });
    }
    None
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
fn ascii_only(bd: &[u8], lo: usize, hi: usize) -> String {
    bd.get(lo..hi.min(bd.len()))
        .unwrap_or(&[])
        .iter()
        .filter(|&&b| (32..127).contains(&b))
        .map(|&b| b as char)
        .collect()
}
/// Longest printable-ASCII run (>=2) — surfaces packed var names / inline strings.
fn ascii_run(bd: &[u8], pos: usize, size: usize) -> Option<String> {
    let slice = bd.get(pos..(pos + size).min(bd.len()))?;
    let (mut best, mut cur) = (String::new(), String::new());
    for &b in slice {
        if (32..127).contains(&b) {
            cur.push(b as char);
        } else {
            if cur.len() > best.len() {
                best = std::mem::take(&mut cur);
            } else {
                cur.clear();
            }
        }
    }
    if cur.len() > best.len() {
        best = cur;
    }
    (best.len() >= 2).then_some(best)
}

// ── misc ─────────────────────────────────────────────────────────────────────────────────────────
fn q(s: &str) -> String {
    format!("{s:?}")
}
fn short(cls: &str) -> &str {
    cls.rsplit('.').next().unwrap_or(cls)
}

const PARAM_TYPES: &[&str] = &[
    "Integer",
    "Boolean",
    "Float",
    "String",
    "Color",
    "ObjectReference",
    "LayerMask",
    "Enum",
    "Vector2",
    "Vector3",
    "Vector4",
    "Rect",
    "Array",
    "Character",
    "AnimationCurve",
    "FsmFloat",
    "FsmInt",
    "FsmBool",
    "FsmString",
    "FsmGameObject",
    "FsmOwnerDefault",
    "FunctionCall",
    "FsmAnimationCurve",
    "FsmEvent",
    "FsmObject",
    "FsmColor",
    "Unsupported",
    "GameObject",
    "FsmVector3",
    "LayoutOption",
    "FsmRect",
    "FsmEventTarget",
    "FsmMaterial",
    "FsmTexture",
    "Quaternion",
    "FsmQuaternion",
    "FsmProperty",
    "FsmVector2",
    "FsmTemplateControl",
    "FsmVar",
    "CustomClass",
    "FsmArray",
    "FsmEnum",
];
fn ptype(i: i32) -> &'static str {
    usize::try_from(i)
        .ok()
        .and_then(|i| PARAM_TYPES.get(i))
        .copied()
        .unwrap_or("?")
}

fn named_vars(v: &FsmVariables) -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    macro_rules! push {
        ($field:ident, $label:literal) => {
            for x in &v.$field {
                if !x.name.is_empty() {
                    out.push((x.name.clone(), $label));
                }
            }
        };
    }
    push!(floatVariables, "float");
    push!(intVariables, "int");
    push!(boolVariables, "bool");
    push!(stringVariables, "string");
    push!(vector2Variables, "vector2");
    push!(vector3Variables, "vector3");
    push!(colorVariables, "color");
    push!(rectVariables, "rect");
    push!(quaternionVariables, "quaternion");
    push!(gameObjectVariables, "gameObject");
    push!(objectVariables, "object");
    push!(materialVariables, "material");
    push!(textureVariables, "texture");
    push!(arrayVariables, "array");
    push!(enumVariables, "enum");
    out
}
