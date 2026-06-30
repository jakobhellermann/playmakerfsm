//! Structured, serializable model of a PlayMaker FSM. Pure data — the decoder that builds it from
//! raw FSM data lives in [`super::decode`].

use rabex_env::component_path::ComponentPath;
use rabex_env::rabex::objects::pptr::PathId;
use std::borrow::Cow;

/// A resolved object pointer: the external file it lives in (if any) plus a stable address.
#[derive(Debug, Clone, Hash, serde::Serialize)]
pub struct ObjectRef {
    pub file: Option<String>,
    pub target: RefTarget,
}

#[derive(Debug, Clone, Hash, serde::Serialize)]
#[serde(tag = "kind", content = "target")]
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
#[derive(Debug, Clone, Hash, serde::Serialize)]
pub enum GoRef {
    /// the FSM's own GameObject (`Owner (Self)`)
    SelfOwner,
    /// bound to a variable, by name
    Var(String),
    /// a concrete object, resolved
    Object(ObjectRef),
}

/// A resolved event target (`FsmEventTarget`).
#[derive(Debug, Clone, Hash, serde::Serialize)]
pub struct EventTarget {
    /// 0 Self, 1 GameObject, 2 GameObjectFSM, 3 FSMComponent, 4 BroadcastAll, 5 HostFSM, 6 SubFSMs
    pub kind: i32,
    pub game_object: GoRef,
    /// target FSM name, if specified
    pub fsm_name: Option<String>,
    /// the targeted PlayMakerFSM component, resolved
    pub fsm: ObjectRef,
    pub exclude_self: bool,
    pub send_to_children: bool,
}

/// A resolved Get/SetProperty target (`FsmProperty`): the object whose property is accessed.
#[derive(Debug, Clone, Hash, serde::Serialize)]
pub struct Property<'a> {
    pub target: GoRef,
    pub type_name: &'a str,
    pub property: &'a str,
    /// `true` for SetProperty, `false` for GetProperty
    pub set: bool,
}

/// An `FsmString` value: a variable binding or a literal.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", content = "value")]
pub enum StrValue {
    Var(String),
    Literal(String),
}

/// An `FsmVar` value: a variable reference or a typed inline constant.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", content = "value")]
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
    Array(ArrayValue),
}

/// An `FsmEnum` value.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", content = "value")]
pub enum EnumValue {
    Var(String),
    Named { enum_name: String, value: i32 },
    Value(i32),
}

/// An `FsmArray` value: a variable reference, or its elements.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", content = "value")]
pub enum ArrayValue {
    Var(String),
    Values(Vec<Value>),
}

/// Any Fsm-wrapped parameter value: a variable binding or a typed literal (objects resolved).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", content = "value")]
pub enum Value {
    Var(String),
    Bool(bool),
    Int(i32),
    Float(f32),
    Str(String),
    Vector(Vec<f32>),
    Enum { enum_name: String, value: i32 },
    Object(ObjectRef),
    Array(ArrayValue),
}

/// A reflective method/property call (`FunctionCall`): the called member, its parameter type, and
/// the value of the active parameter (selected by `parameter_type`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Call<'a> {
    pub function: &'a str,
    pub parameter_type: &'a str,
    pub value: Option<Value>,
}

/// An animation curve (`FsmAnimationCurve`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Curve {
    pub keys: Vec<CurveKey>,
    pub pre_infinity: i32,
    pub post_infinity: i32,
    pub rotation_order: i32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CurveKey {
    pub time: f32,
    pub value: f32,
    pub in_slope: f32,
    pub out_slope: f32,
    pub in_weight: f32,
    pub out_weight: f32,
    pub weighted_mode: i32,
}

/// A template variable binding (`FsmVarOverride`): a template variable and the value bound to it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VarOverride {
    pub variable: String,
    pub value: VarValue,
}

/// Structured view of a whole FSM.
#[derive(serde::Serialize)]
pub struct FsmModel<'a> {
    pub name: &'a str,
    pub start_state: &'a str,
    pub events: Vec<Event<'a>>,
    pub global_transitions: Vec<Transition<'a>>,
    pub states: Vec<State<'a>>,
    pub variables: Vec<Variable<'a>>,
}

/// A declared FSM event.
#[derive(serde::Serialize)]
pub struct Event<'a> {
    pub name: &'a str,
    pub is_global: bool,
    pub is_system: bool,
}

#[derive(serde::Serialize)]
pub struct Transition<'a> {
    pub event: &'a str,
    pub to_state: &'a str,
}

/// The state's node rectangle in the PlayMaker editor graph (raw authored layout).
#[derive(serde::Serialize)]
pub struct StatePos {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(serde::Serialize)]
pub struct State<'a> {
    pub name: &'a str,
    pub is_start: bool,
    pub position: StatePos,
    pub transitions: Vec<Transition<'a>>,
    pub actions: Vec<Action<'a>>,
}

#[derive(serde::Serialize)]
pub struct Action<'a> {
    /// Full action class name (e.g. `HutongGames.PlayMaker.Actions.SetBoolValue`).
    pub class: &'a str,
    /// User-given label from the PlayMaker editor
    pub custom_name: Option<&'a str>,
    pub enabled: bool,
    pub params: Vec<Param<'a>>,
}

#[derive(serde::Serialize)]
pub struct Param<'a> {
    /// Field name; empty for unnamed params (e.g. array elements).
    pub name: &'a str,
    /// PlayMaker parameter type name (see [`PARAM_TYPES`]).
    pub type_name: &'static str,
    pub value: ParamValue<'a>,
}

/// A decoded parameter value.
#[derive(serde::Serialize)]
#[serde(tag = "type", content = "value")]
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
    /// A plain C# enum param resolved to its member name (e.g. `operation` -> "Multiply"). The enum
    /// type isn't in the FSM data, so this is filled in at content-build time from the game assembly.
    EnumMember(Cow<'a, str>),
    /// A layer-index param resolved to its Unity layer name (from the TagManager). Like
    /// [`EnumMember`](Self::EnumMember), the names aren't in the FSM data, so this is filled in at
    /// content-build time. `name` is `None` when the index has no named layer.
    Layer {
        index: i32,
        name: Option<Cow<'a, str>>,
    },
    Array(ArrayValue),
    Property(Property<'a>),
    AnimCurve(Curve),
    /// An inline `Array` param: its element params, nested.
    List(Vec<Param<'a>>),
    /// Unity object reference (`ObjectReference`/`GameObject`), resolved to a stable address.
    Pptr(ObjectRef),

    /// Couldn't decode (unknown type / index out of range / truncated).
    Raw(&'a [u8]),
}

#[derive(serde::Serialize)]
pub struct Variable<'a> {
    pub name: &'a str,
    pub category: &'static str,
}

/// A RunFSM template binding: the template to run plus how the parent FSM's variables and
/// events are wired into it. Variable bindings are either directional ([`inputs`](Self::inputs)
/// into the template, [`outputs`](Self::outputs) back out) or undirected
/// ([`overrides`](Self::overrides)); the unused lists are empty.
#[derive(serde::Serialize)]
pub struct TemplateControl<'a> {
    pub template: PathId,
    pub inputs: Vec<VarOverride>,
    pub outputs: Vec<VarOverride>,
    pub overrides: Vec<VarOverride>,
    pub events: Vec<(&'a str, &'a str)>,
}
