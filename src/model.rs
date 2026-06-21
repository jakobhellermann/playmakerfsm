//! A structured, borrowed view over a [`Fsm`]

// ActionData stores all params of a state flat across `paramName/paramDataType/paramDataPos`, sliced
// per action by `actionStartIndex` (no trailing sentinel).
// - feference params (FsmString, FsmOwnerDefault, FsmVar, …) live in a typed array that `paramDataPos` indexes
// - value primitives (Boolean/Integer/Float and the packed Fsm-wrappers/FsmEvent) live in the flat `byteData`
// at byte-offset `paramDataPos` (`paramByteDataSize` = length).
// The Fsm scalar/value wrappers pack as `[value(n)][useVariable(1)][name(rest)]`.

use crate::raw::*;
use rabex_env::rabex::objects::PPtr;

/// Structured view of a whole FSM.
pub struct FsmModel<'a> {
    pub name: &'a str,
    pub start_state: &'a str,
    pub event_count: usize,
    pub global_transitions: Vec<Transition<'a>>,
    pub states: Vec<State<'a>>,
    pub variables: Vec<Variable<'a>>,
}

pub struct Transition<'a> {
    pub event: &'a str,
    pub to_state: &'a str,
}

pub struct State<'a> {
    pub name: &'a str,
    pub is_start: bool,
    pub transitions: Vec<Transition<'a>>,
    pub actions: Vec<Action<'a>>,
}

pub struct Action<'a> {
    /// Full action class name (e.g. `HutongGames.PlayMaker.Actions.SetBoolValue`).
    pub class: &'a str,
    /// User-given label from the PlayMaker editor
    pub custom_name: Option<&'a str>,
    pub enabled: bool,
    pub params: Vec<Param<'a>>,
}

pub struct Param<'a> {
    /// Field name; empty for unnamed params (e.g. array elements).
    pub name: &'a str,
    /// PlayMaker parameter type name (see [`PARAM_TYPES`]).
    pub type_name: &'static str,
    pub value: ParamValue<'a>,
}

/// A decoded parameter value.
pub enum ParamValue<'a> {
    Bool(bool),
    Int(i32),
    Float(f32),
    /// Vector2/3, Quaternion, Color or Rect components.
    Vector(Vec<f32>),
    /// A packed wrapper bound to a variable; `None` = bound but unnamed (`<var>`).
    PackedVar(Option<String>),
    /// FsmEvent param; `None` = no event (`(none)`).
    Event(Option<String>),

    Str(&'a str),
    FsmString(&'a FsmString),
    Owner(&'a FsmOwnerDefault),
    Var(&'a FsmVar),
    GameObject(&'a FsmGameObject),
    Object(&'a FsmObject),
    EventTarget(&'a FsmEventTarget),
    Function(&'a FunctionCall),
    Template(&'a FsmTemplateControl),
    Enum(&'a FsmEnum),
    Array(&'a FsmArray),
    Property(&'a FsmProperty),
    ArraySize(i32),
    /// Unity object reference (`ObjectReference`/`GameObject`).
    Pptr(&'a PPtr),

    /// Couldn't decode (unknown type / index out of range / truncated).
    Raw(&'a [u8]),
}

pub struct Variable<'a> {
    pub name: &'a str,
    pub category: &'static str,
}

/// Resolve a [`Fsm`] into the structured [`FsmModel`].
pub fn decode_fsm(fsm: &Fsm) -> FsmModel<'_> {
    FsmModel {
        name: &fsm.name,
        start_state: &fsm.startState,
        event_count: fsm.events.len(),
        global_transitions: fsm.globalTransitions.iter().map(transition).collect(),
        states: fsm
            .states
            .iter()
            .map(|s| decode_state(s, &fsm.startState))
            .collect(),
        variables: decode_variables(&fsm.variables),
    }
}

fn transition(t: &FsmTransition) -> Transition<'_> {
    Transition {
        event: &t.fsmEvent.name,
        to_state: &t.toState,
    }
}

fn decode_state<'a>(s: &'a FsmState, start: &str) -> State<'a> {
    let ad = &s.actionData;
    let actions = ad
        .actionNames
        .iter()
        .enumerate()
        .map(|(ai, cls)| Action {
            class: cls,
            custom_name: ad
                .customNames
                .get(ai)
                .filter(|c| !c.is_empty())
                .map(String::as_str),
            enabled: ad.actionEnabled.get(ai) != Some(&0),
            params: decode_params(ad, ai),
        })
        .collect();
    State {
        name: &s.name,
        is_start: s.name == start,
        transitions: s.transitions.iter().map(transition).collect(),
        actions,
    }
}

/// Decode action `ai`'s parameter slice into typed [`Param`]s.
fn decode_params(ad: &ActionData, ai: usize) -> Vec<Param<'_>> {
    let starts = &ad.actionStartIndex;
    let Some(&lo) = starts.get(ai) else {
        return vec![];
    };
    // actionStartIndex has no end sentinel: the last action's params run to the end of the arrays.
    let hi = starts
        .get(ai + 1)
        .map(|&x| x as usize)
        .unwrap_or(ad.paramName.len());

    (lo as usize..hi)
        .filter_map(|j| {
            let dt = *ad.paramDataType.get(j)?;
            let pos = *ad.paramDataPos.get(j)? as usize;
            let size = ad.paramByteDataSize.get(j).copied().unwrap_or(0) as usize;
            let type_name = ptype(dt);
            let name = ad.paramName.get(j).map(String::as_str).unwrap_or("");
            Some(Param {
                name,
                type_name,
                value: decode_param(ad, type_name, pos, size),
            })
        })
        .collect()
}

fn decode_param<'a>(
    ad: &'a ActionData,
    type_name: &str,
    pos: usize,
    size: usize,
) -> ParamValue<'a> {
    let bd = &ad.byteData;
    let decoded = match type_name {
        "FsmString" => ad.fsmStringParams.get(pos).map(ParamValue::FsmString),
        "String" => ad.stringParams.get(pos).map(|s| ParamValue::Str(s)),
        "FsmOwnerDefault" => ad.fsmOwnerDefaultParams.get(pos).map(ParamValue::Owner),
        "FsmVar" => ad.fsmVarParams.get(pos).map(ParamValue::Var),
        "FsmGameObject" => ad.fsmGameObjectParams.get(pos).map(ParamValue::GameObject),
        "FsmObject" | "FsmMaterial" | "FsmTexture" => {
            ad.fsmObjectParams.get(pos).map(ParamValue::Object)
        }
        "FsmEventTarget" => ad
            .fsmEventTargetParams
            .get(pos)
            .map(ParamValue::EventTarget),
        "FunctionCall" => ad.functionCallParams.get(pos).map(ParamValue::Function),
        "FsmTemplateControl" => ad
            .fsmTemplateControlParams
            .get(pos)
            .map(ParamValue::Template),
        "Array" => ad
            .arrayParamSizes
            .get(pos)
            .copied()
            .map(ParamValue::ArraySize),
        "ObjectReference" | "GameObject" => ad.unityObjectParams.get(pos).map(ParamValue::Pptr),
        "Boolean" => Some(ParamValue::Bool(bd.get(pos).copied().unwrap_or(0) != 0)),
        "Integer" | "Enum" | "LayerMask" => read_i32(bd, pos).map(ParamValue::Int),
        "Float" => read_f32(bd, pos).map(ParamValue::Float),
        "FsmBool" => Some(packed_scalar(bd, pos, size, Packed::Bool)),
        "FsmInt" => Some(packed_scalar(bd, pos, size, Packed::Int)),
        "FsmFloat" => Some(packed_scalar(bd, pos, size, Packed::Float)),
        // value-type wrappers pack their floats into byteData (size == n*4 + useVariable byte [+ name]).
        "FsmVector2" => Some(packed_vec(bd, pos, size, 2)),
        "FsmVector3" => Some(packed_vec(bd, pos, size, 3)),
        "FsmQuaternion" | "FsmColor" | "FsmRect" => Some(packed_vec(bd, pos, size, 4)),
        // these carry no byteData (size 0): paramDataPos indexes the typed param array instead.
        "FsmEnum" => ad.fsmEnumParams.get(pos).map(ParamValue::Enum),
        "FsmArray" => ad.fsmArrayParams.get(pos).map(ParamValue::Array),
        "FsmProperty" => ad.fsmPropertyParams.get(pos).map(ParamValue::Property),
        "FsmEvent" => Some(ParamValue::Event(if size == 0 {
            None
        } else {
            longest_ascii_run(byte_slice(bd, pos, size))
        })),
        _ => None,
    };
    decoded.unwrap_or_else(|| ParamValue::Raw(byte_slice(bd, pos, size)))
}

#[derive(Clone, Copy)]
enum Packed {
    Bool,
    Int,
    Float,
}

/// Fsm scalar wrapper: `[value(valsize)][useVariable(1)][name(rest)]`. The wrapper type tells us how to
/// read the value bytes — int vs float can't be guessed (a packed `1` would read as a denormal float).
fn packed_scalar(bd: &[u8], pos: usize, size: usize, kind: Packed) -> ParamValue<'_> {
    let valsize = match kind {
        Packed::Bool => 1,
        Packed::Int | Packed::Float => 4,
    };
    if let Some(name) = packed_var(bd, pos, size, valsize) {
        return ParamValue::PackedVar(name);
    }
    if size < valsize {
        return ParamValue::Raw(byte_slice(bd, pos, size));
    }
    match kind {
        Packed::Bool => ParamValue::Bool(bd.get(pos).copied().unwrap_or(0) != 0),
        Packed::Int => ParamValue::Int(read_i32(bd, pos).unwrap_or(0)),
        Packed::Float => ParamValue::Float(read_f32(bd, pos).unwrap_or(0.0)),
    }
}

/// Fsm value-type wrapper packing `n` floats as `[f32 × n][useVariable(1)][name(rest)]`.
fn packed_vec(bd: &[u8], pos: usize, size: usize, n: usize) -> ParamValue<'_> {
    let valsize = n * 4;
    if let Some(name) = packed_var(bd, pos, size, valsize) {
        return ParamValue::PackedVar(name);
    }
    if size < valsize {
        return ParamValue::Raw(byte_slice(bd, pos, size));
    }
    let comps = (0..n)
        .map(|i| read_f32(bd, pos + i * 4).unwrap_or(0.0))
        .collect();
    ParamValue::Vector(comps)
}

/// If the useVariable byte after the value is set, the wrapper is a variable reference: returns
/// `Some(Some(name))`, or `Some(None)` when the bound name is empty. `None` = inline value.
fn packed_var(bd: &[u8], pos: usize, size: usize, valsize: usize) -> Option<Option<String>> {
    if size > valsize && bd.get(pos + valsize).copied().unwrap_or(0) != 0 {
        let name = ascii_only(bd, pos + valsize + 1, pos + size);
        Some((!name.is_empty()).then_some(name))
    } else {
        None
    }
}

fn read_i32(bd: &[u8], pos: usize) -> Option<i32> {
    bd.get(pos..pos + 4)
        .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
}
fn read_f32(bd: &[u8], pos: usize) -> Option<f32> {
    bd.get(pos..pos + 4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
}
fn ascii_only(bd: &[u8], lo: usize, hi: usize) -> String {
    bd.get(lo..hi.min(bd.len()))
        .unwrap_or(&[])
        .iter()
        .filter(|&&b| (32..127).contains(&b))
        .map(|&b| b as char)
        .collect()
}
/// byteData sub-slice `[pos, pos+size)`, clamped to bounds.
fn byte_slice(bd: &[u8], pos: usize, size: usize) -> &[u8] {
    bd.get(pos..(pos + size).min(bd.len())).unwrap_or(&[])
}
/// Longest printable-ASCII run (>=2) in `bytes` — surfaces packed var names / inline strings hidden in
/// otherwise-undecoded byteData. Useful for interpreting a [`ParamValue::Raw`] slice.
pub fn longest_ascii_run(bytes: &[u8]) -> Option<String> {
    let (mut best, mut cur) = (String::new(), String::new());
    for &b in bytes {
        if (32..127).contains(&b) {
            cur.push(b as char);
        } else if cur.len() > best.len() {
            best = std::mem::take(&mut cur);
        } else {
            cur.clear();
        }
    }
    if cur.len() > best.len() {
        best = cur;
    }
    (best.len() >= 2).then_some(best)
}

fn decode_variables(v: &FsmVariables) -> Vec<Variable<'_>> {
    let mut out = Vec::new();
    macro_rules! push {
        ($field:ident, $label:literal) => {
            for x in &v.$field {
                if !x.name.is_empty() {
                    out.push(Variable {
                        name: &x.name,
                        category: $label,
                    });
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

/// PlayMaker `ParamDataType` ordinal -> name. `paramDataType` indexes into this.
pub const PARAM_TYPES: &[&str] = &[
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

pub fn ptype(i: i32) -> &'static str {
    usize::try_from(i)
        .ok()
        .and_then(|i| PARAM_TYPES.get(i))
        .copied()
        .unwrap_or("?")
}
