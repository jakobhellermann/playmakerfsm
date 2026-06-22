//! A structured, borrowed view over a [`Fsm`]

// ActionData stores all params of a state flat across `paramName/paramDataType/paramDataPos`, sliced
// per action by `actionStartIndex` (no trailing sentinel).
// - feference params (FsmString, FsmOwnerDefault, FsmVar, …) live in a typed array that `paramDataPos` indexes
// - value primitives (Boolean/Integer/Float and the packed Fsm-wrappers/FsmEvent) live in the flat `byteData`
// at byte-offset `paramDataPos` (`paramByteDataSize` = length).
// The Fsm scalar/value wrappers pack as `[value(n)][useVariable(1)][name(rest)]`.

use crate::raw::*;
use rabex_env::component_path::ComponentPath;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::qualify::Qualifier;
use rabex_env::rabex::objects::PPtr;
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;
use std::borrow::Cow;

/// Decoding context: resolves object pointers to stable [`ObjectRef`]s. Created per serialized file
/// and reused across the FSMs in it.
pub struct Context<'a, R, P> {
    qualifier: Qualifier<'a, R, P>,
}

impl<'a, R: EnvResolver, P: TypeTreeProvider> Context<'a, R, P> {
    pub fn new(handle: &SerializedFileHandle<'a, R, P>) -> Self {
        Context {
            qualifier: Qualifier::new(handle),
        }
    }

    fn resolve(&mut self, pptr: PPtr) -> ObjectRef {
        let qualified = self.qualifier.qualify(pptr);
        let target = match qualified.path {
            Some(path) => RefTarget::Path(path),
            None if pptr.m_PathID == 0 => RefTarget::Null,
            None => RefTarget::Loose {
                name: qualified.name,
                id: pptr.m_PathID,
            },
        };
        ObjectRef {
            file: qualified.file,
            target,
        }
    }

    /// An object value that is either bound to a variable (`use_var`) or a concrete pointer.
    fn go_ref(&mut self, use_var: u8, name: &str, value: PPtr) -> GoRef {
        if use_var != 0 {
            GoRef::Var(name.to_owned())
        } else {
            GoRef::Object(self.resolve(value))
        }
    }

    fn owner_ref(&mut self, owner: &FsmOwnerDefault) -> GoRef {
        if owner.ownerOption == 0 {
            GoRef::SelfOwner
        } else {
            self.go_ref(
                owner.gameObject.useVariable,
                &owner.gameObject.name,
                owner.gameObject.value,
            )
        }
    }

    fn event_target(&mut self, t: &FsmEventTarget) -> EventTarget {
        EventTarget {
            kind: t.target,
            game_object: self.owner_ref(&t.gameObject),
            fsm_name: (!t.fsmName.value.is_empty()).then(|| t.fsmName.value.clone()),
            fsm: self.resolve(PPtr::new(t.fsmComponent.m_FileID, t.fsmComponent.m_PathID)),
        }
    }

    /// An `FsmVar` value: a variable reference, or a typed inline constant (objects resolved).
    fn var_value(&mut self, v: &FsmVar) -> VarValue {
        if !v.variableName.is_empty() {
            return VarValue::Var(v.variableName.clone());
        }
        if v.useVariable != 0 {
            return VarValue::Unset;
        }
        // VariableType: 0 Float 1 Int 2 Bool 3 GameObject 4 String 5-8/11 Vector 14 Enum, -1 unused.
        match v.r#type {
            -1 => VarValue::Unused,
            0 => VarValue::Float(v.floatValue),
            1 => VarValue::Int(v.intValue),
            2 => VarValue::Bool(v.boolValue != 0),
            4 => VarValue::Str(v.stringValue.clone()),
            14 => VarValue::Enum(v.intValue),
            3 | 9 | 10 | 12 => VarValue::Object(self.resolve(v.objectReference)),
            5 | 6 | 7 | 8 | 11 => {
                let w = &v.vector4Value;
                VarValue::Vector(vec![w.x, w.y, w.z, w.w])
            }
            _ => VarValue::Inline,
        }
    }

    fn array_value(&mut self, a: &FsmArray) -> ArrayValue {
        if a.useVariable != 0 && !a.name.is_empty() {
            return ArrayValue::Var(a.name.clone());
        }
        let objects = a
            .objectReferences
            .iter()
            .map(|&p| self.resolve(p))
            .collect();
        let len = [
            a.floatValues.len(),
            a.intValues.len(),
            a.boolValues.len(),
            a.stringValues.len(),
            a.vector4Values.len(),
            a.objectReferences.len(),
        ]
        .into_iter()
        .max()
        .unwrap_or(0);
        ArrayValue::Values { len, objects }
    }

    fn var_override(&mut self, o: &FsmVarOverride) -> VarOverride {
        VarOverride {
            variable: o.variable.name.clone(),
            value: self.var_value(&o.fsmVar),
        }
    }

    fn var_overrides(&mut self, v: &Option<Vec<FsmVarOverride>>) -> Vec<VarOverride> {
        let Some(list) = v.as_deref() else {
            return Vec::new();
        };
        list.iter().map(|o| self.var_override(o)).collect()
    }

    fn template_control<'b>(&mut self, t: &'b FsmTemplateControl) -> TemplateControl<'b> {
        // the template pointer is stored under one of two field names across encodings
        let template = t
            .target
            .as_ref()
            .map(|p| p.m_PathID)
            .or_else(|| t.fsmTemplate.as_ref().map(|p| p.m_PathID))
            .unwrap_or_default();
        TemplateControl {
            template,
            inputs: self.var_overrides(&t.inputVariables),
            outputs: self.var_overrides(&t.outputVariables),
            overrides: self.var_overrides(&t.fsmVarOverrides),
            events: t
                .outputEvents
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|e| (e.fromEvent.name.as_str(), e.toEvent.name.as_str()))
                .collect(),
        }
    }
}

/// A resolved object pointer: the external file it lives in (if any) plus a stable address.
#[derive(Debug, Clone, Hash)]
pub struct ObjectRef {
    pub file: Option<String>,
    pub target: RefTarget,
}

#[derive(Debug, Clone, Hash)]
pub enum RefTarget {
    /// In the scene hierarchy, addressable by a (version-stable) component path.
    Path(ComponentPath),
    /// A loose object (asset / component without a GameObject): no stable path, but its `m_Name`
    /// when available, plus its path id.
    Loose { name: Option<String>, id: PathId },
    /// A null pointer.
    Null,
}

/// An object-valued parameter (`FsmGameObject`/`FsmObject`/`FsmOwnerDefault`): a variable binding, a
/// resolved object pointer, or — for owner params — the FSM's own GameObject.
#[derive(Debug, Clone, Hash)]
pub enum GoRef {
    /// the FSM's own GameObject (`Owner (Self)`)
    SelfOwner,
    /// bound to a variable, by name
    Var(String),
    /// a concrete object, resolved
    Object(ObjectRef),
}

/// A resolved event target (`FsmEventTarget`).
#[derive(Debug, Clone, Hash)]
pub struct EventTarget {
    /// 0 Self, 1 GameObject, 2 GameObjectFSM, 3 FSMComponent, 4 BroadcastAll, 5 HostFSM, 6 SubFSMs
    pub kind: i32,
    pub game_object: GoRef,
    /// target FSM name, if specified
    pub fsm_name: Option<String>,
    /// the targeted PlayMakerFSM component, resolved
    pub fsm: ObjectRef,
}

/// A resolved Get/SetProperty target (`FsmProperty`): the object whose property is accessed.
#[derive(Debug, Clone, Hash)]
pub struct Property<'a> {
    pub target: GoRef,
    pub type_name: &'a str,
    pub property: &'a str,
}

/// An `FsmString` value: a variable binding or a literal.
#[derive(Debug, Clone)]
pub enum StrValue {
    Var(String),
    Literal(String),
}

/// An `FsmVar` value: a variable reference or a typed inline constant.
#[derive(Debug, Clone)]
pub enum VarValue {
    /// bound to a named variable
    Var(String),
    /// bound to a variable, unnamed
    Unset,
    Unused,
    Float(f32),
    Int(i32),
    Bool(bool),
    Str(String),
    Object(ObjectRef),
    Vector(Vec<f32>),
    Enum(i32),
    /// an inline constant of an unmodeled type
    Inline,
}

/// An `FsmEnum` value.
#[derive(Debug, Clone)]
pub enum EnumValue {
    Var(String),
    Named { enum_name: String, value: i32 },
    Value(i32),
}

/// An `FsmArray` value: a variable reference, or its element count plus any resolved object elements.
#[derive(Debug, Clone)]
pub enum ArrayValue {
    Var(String),
    Values { len: usize, objects: Vec<ObjectRef> },
}

/// A reflective method/property call (`FunctionCall`). Parameter values are not modeled.
#[derive(Debug, Clone)]
pub struct Call<'a> {
    pub function: &'a str,
    pub parameter_type: &'a str,
}

/// An animation curve (`FsmAnimationCurve`).
#[derive(Debug, Clone)]
pub struct Curve {
    pub keys: Vec<CurveKey>,
}

#[derive(Debug, Clone)]
pub struct CurveKey {
    pub time: f32,
    pub value: f32,
    pub in_slope: f32,
    pub out_slope: f32,
}

/// A template variable binding (`FsmVarOverride`): a template variable and the value bound to it.
#[derive(Debug, Clone)]
pub struct VarOverride {
    pub variable: String,
    pub value: VarValue,
}

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

    /// A raw `String` param: borrowed from `stringParams` (typed form) or owned (byteData form).
    Str(Cow<'a, str>),
    FsmString(StrValue),
    Owner(GoRef),
    Var(VarValue),
    GameObject(GoRef),
    Object(GoRef),
    EventTarget(EventTarget),
    Function(Call<'a>),
    Template(TemplateControl<'a>),
    Enum(EnumValue),
    Array(ArrayValue),
    Property(Property<'a>),
    AnimCurve(Curve),
    ArraySize(i32),
    /// Unity object reference (`ObjectReference`/`GameObject`), resolved to a stable address.
    Pptr(ObjectRef),

    /// Couldn't decode (unknown type / index out of range / truncated).
    Raw(&'a [u8]),
}

pub struct Variable<'a> {
    pub name: &'a str,
    pub category: &'static str,
}

/// A RunFSM template binding: the template to run plus how the parent FSM's variables and
/// events are wired into it. Variable bindings are either directional ([`inputs`](Self::inputs)
/// into the template, [`outputs`](Self::outputs) back out) or undirected
/// ([`overrides`](Self::overrides)); the unused lists are empty.
pub struct TemplateControl<'a> {
    pub template: PathId,
    pub inputs: Vec<VarOverride>,
    pub outputs: Vec<VarOverride>,
    pub overrides: Vec<VarOverride>,
    pub events: Vec<(&'a str, &'a str)>,
}

/// Resolve a [`Fsm`] into the structured [`FsmModel`], resolving object pointers via `ctx`.
pub fn decode_fsm<'a, R: EnvResolver, P: TypeTreeProvider>(
    fsm: &'a Fsm,
    ctx: &mut Context<'_, R, P>,
) -> FsmModel<'a> {
    FsmModel {
        name: &fsm.name,
        start_state: &fsm.startState,
        event_count: fsm.events.len(),
        global_transitions: fsm.globalTransitions.iter().map(transition).collect(),
        states: fsm
            .states
            .iter()
            .map(|s| decode_state(s, &fsm.startState, &mut *ctx))
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

fn decode_state<'a, R: EnvResolver, P: TypeTreeProvider>(
    s: &'a FsmState,
    start: &str,
    ctx: &mut Context<'_, R, P>,
) -> State<'a> {
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
            params: decode_params(ad, ai, &mut *ctx),
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
fn decode_params<'a, R: EnvResolver, P: TypeTreeProvider>(
    ad: &'a ActionData,
    ai: usize,
    ctx: &mut Context<'_, R, P>,
) -> Vec<Param<'a>> {
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
                value: decode_param(ad, type_name, pos, size, &mut *ctx),
            })
        })
        .collect()
}

fn decode_param<'a, R: EnvResolver, P: TypeTreeProvider>(
    ad: &'a ActionData,
    type_name: &str,
    pos: usize,
    size: usize,
    ctx: &mut Context<'_, R, P>,
) -> ParamValue<'a> {
    let bd = &ad.byteData;
    let decoded = match type_name {
        "FsmString" => ad
            .fsmStringParams
            .get(pos)
            .map(|s| ParamValue::FsmString(str_value(s))),
        // raw String has the same dual encoding: the bytes inline (size > 0) or `stringParams[pos]`.
        "String" if size > 0 => Some(ParamValue::Str(
            String::from_utf8_lossy(byte_slice(bd, pos, size))
                .into_owned()
                .into(),
        )),
        "String" => ad
            .stringParams
            .get(pos)
            .map(|s| ParamValue::Str(Cow::Borrowed(s))),
        // raw (non-Fsm) value types: plain floats packed in byteData, no useVariable byte.
        "Vector2" => raw_floats(bd, pos, 2),
        "Vector3" => raw_floats(bd, pos, 3),
        "Vector4" | "Color" | "Rect" | "Quaternion" => raw_floats(bd, pos, 4),
        "FsmOwnerDefault" => ad
            .fsmOwnerDefaultParams
            .get(pos)
            .map(|o| ParamValue::Owner(ctx.owner_ref(o))),
        "FsmVar" => ad
            .fsmVarParams
            .get(pos)
            .map(|fv| ParamValue::Var(ctx.var_value(fv))),
        "FsmGameObject" => ad
            .fsmGameObjectParams
            .get(pos)
            .map(|g| ParamValue::GameObject(ctx.go_ref(g.useVariable, &g.name, g.value))),
        "FsmObject" | "FsmMaterial" | "FsmTexture" => ad
            .fsmObjectParams
            .get(pos)
            .map(|o| ParamValue::Object(ctx.go_ref(o.useVariable, &o.name, o.value))),
        "FsmEventTarget" => ad
            .fsmEventTargetParams
            .get(pos)
            .map(|t| ParamValue::EventTarget(ctx.event_target(t))),
        "FunctionCall" => ad.functionCallParams.get(pos).map(|f| {
            ParamValue::Function(Call {
                function: &f.FunctionName,
                parameter_type: &f.parameterType,
            })
        }),
        "FsmTemplateControl" => ad
            .fsmTemplateControlParams
            .get(pos)
            .map(|t| ParamValue::Template(ctx.template_control(t))),
        "Array" => ad
            .arrayParamSizes
            .get(pos)
            .copied()
            .map(ParamValue::ArraySize),
        "ObjectReference" | "GameObject" => ad
            .unityObjectParams
            .get(pos)
            .map(|p| ParamValue::Pptr(ctx.resolve(*p))),
        "Boolean" => Some(ParamValue::Bool(bd.get(pos).copied().unwrap_or(0) != 0)),
        "Integer" | "Enum" | "LayerMask" => read_i32(bd, pos).map(ParamValue::Int),
        "Float" => read_f32(bd, pos).map(ParamValue::Float),
        // Scalar/value wrappers have two encodings: byteData-packed (size > 0, `paramDataPos` = byte
        // offset, `[value][useVariable][name]`) or — far more common — the typed param array (size 0,
        // `paramDataPos` = array index). Both encode the same {useVariable, name, value}.
        "FsmBool" if size > 0 => Some(packed_scalar(bd, pos, size, Packed::Bool)),
        "FsmInt" if size > 0 => Some(packed_scalar(bd, pos, size, Packed::Int)),
        "FsmFloat" if size > 0 => Some(packed_scalar(bd, pos, size, Packed::Float)),
        "FsmVector2" if size > 0 => Some(packed_vec(bd, pos, size, 2)),
        "FsmVector3" if size > 0 => Some(packed_vec(bd, pos, size, 3)),
        "FsmQuaternion" | "FsmColor" | "FsmRect" if size > 0 => Some(packed_vec(bd, pos, size, 4)),
        "FsmBool" => ad
            .fsmBoolParams
            .get(pos)
            .map(|f| wrap(f.useVariable, &f.name, ParamValue::Bool(f.value != 0))),
        "FsmInt" => ad
            .fsmIntParams
            .get(pos)
            .map(|f| wrap(f.useVariable, &f.name, ParamValue::Int(f.value))),
        "FsmFloat" => ad
            .fsmFloatParams
            .get(pos)
            .map(|f| wrap(f.useVariable, &f.name, ParamValue::Float(f.value))),
        "FsmVector2" => ad.fsmVector2Params.get(pos).map(|v| {
            wrap(
                v.useVariable,
                &v.name,
                ParamValue::Vector(vec![v.value.x, v.value.y]),
            )
        }),
        "FsmVector3" => ad.fsmVector3Params.get(pos).map(|v| {
            wrap(
                v.useVariable,
                &v.name,
                ParamValue::Vector(vec![v.value.x, v.value.y, v.value.z]),
            )
        }),
        "FsmQuaternion" => ad.fsmQuaternionParams.get(pos).map(|v| {
            wrap(
                v.useVariable,
                &v.name,
                ParamValue::Vector(vec![v.value.x, v.value.y, v.value.z, v.value.w]),
            )
        }),
        "FsmColor" => ad.fsmColorParams.get(pos).map(|v| {
            wrap(
                v.useVariable,
                &v.name,
                ParamValue::Vector(vec![v.value.r, v.value.g, v.value.b, v.value.a]),
            )
        }),
        "FsmRect" => ad.fsmRectParams.get(pos).map(|v| {
            wrap(
                v.useVariable,
                &v.name,
                ParamValue::Vector(vec![v.value.x, v.value.y, v.value.width, v.value.height]),
            )
        }),
        // these always use the typed param array (no byteData form).
        "FsmEnum" => ad
            .fsmEnumParams
            .get(pos)
            .map(|e| ParamValue::Enum(enum_value(e))),
        "FsmArray" => ad
            .fsmArrayParams
            .get(pos)
            .map(|a| ParamValue::Array(ctx.array_value(a))),
        "FsmProperty" => ad.fsmPropertyParams.get(pos).map(|p| {
            ParamValue::Property(Property {
                target: ctx.go_ref(
                    p.TargetObject.useVariable,
                    &p.TargetObject.name,
                    p.TargetObject.value,
                ),
                type_name: &p.TargetTypeName,
                property: &p.PropertyName,
            })
        }),
        "FsmAnimationCurve" => ad
            .animationCurveParams
            .get(pos)
            .map(|c| ParamValue::AnimCurve(curve(c))),
        "FsmEvent" => Some(ParamValue::Event(if size == 0 {
            None
        } else {
            longest_ascii_run(byte_slice(bd, pos, size))
        })),
        _ => None,
    };
    decoded.unwrap_or_else(|| ParamValue::Raw(byte_slice(bd, pos, size)))
}

/// Normalize a typed-array Fsm scalar/value wrapper: a variable binding becomes [`ParamValue::PackedVar`]
/// (`None` if the bound name is empty), otherwise the supplied inline `value` — mirroring the byteData
/// `[value][useVariable][name]` decode so both encodings render identically.
fn wrap<'a>(use_var: u8, name: &str, value: ParamValue<'a>) -> ParamValue<'a> {
    if use_var != 0 {
        ParamValue::PackedVar((!name.is_empty()).then(|| name.to_string()))
    } else {
        value
    }
}

/// An `FsmString`: a variable binding or a literal.
fn str_value(s: &FsmString) -> StrValue {
    if s.useVariable != 0 && !s.name.is_empty() {
        StrValue::Var(s.name.clone())
    } else {
        StrValue::Literal(s.value.clone())
    }
}

fn enum_value(e: &FsmEnum) -> EnumValue {
    if e.useVariable != 0 && !e.name.is_empty() {
        EnumValue::Var(e.name.clone())
    } else if !e.enumName.is_empty() {
        EnumValue::Named {
            enum_name: e.enumName.clone(),
            value: e.intValue,
        }
    } else {
        EnumValue::Value(e.intValue)
    }
}

fn curve(c: &FsmAnimationCurve) -> Curve {
    Curve {
        keys: c
            .curve
            .m_Curve
            .iter()
            .map(|k| CurveKey {
                time: k.time,
                value: k.value,
                in_slope: k.inSlope,
                out_slope: k.outSlope,
            })
            .collect(),
    }
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

/// Raw value-type vector: `n` consecutive f32s in byteData, no useVariable byte. `None` if truncated.
fn raw_floats(bd: &[u8], pos: usize, n: usize) -> Option<ParamValue<'static>> {
    (0..n)
        .map(|i| read_f32(bd, pos + i * 4))
        .collect::<Option<Vec<f32>>>()
        .map(ParamValue::Vector)
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
