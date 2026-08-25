//! Structured, serializable model of a PlayMaker FSM. Pure data — the decoder that builds it from
//! raw FSM data lives in [`super::decode`].

use rabex_env::component_path::ComponentPath;
use rabex_env::rabex::objects::pptr::PathId;
use std::borrow::Cow;

/// A resolved object pointer: the external file it lives in (if any) plus a stable address.
#[derive(Debug, Clone, Hash, serde::Serialize, serde::Deserialize)]
pub struct ObjectRef {
    pub file: Option<String>,
    pub target: RefTarget,
}

#[derive(Debug, Clone, Hash, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, Hash, serde::Serialize, serde::Deserialize)]
pub enum GoRef {
    /// the FSM's own GameObject (`Owner (Self)`)
    SelfOwner,
    /// bound to a variable, by name
    Var(String),
    /// a concrete object, resolved
    Object(ObjectRef),
}

/// A resolved event target (`FsmEventTarget`).
#[derive(Debug, Clone, Hash, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, Hash, serde::Serialize, serde::Deserialize)]
pub struct Property<'a> {
    pub target: GoRef,
    pub type_name: Cow<'a, str>,
    pub property: Cow<'a, str>,
    /// `true` for SetProperty, `false` for GetProperty
    pub set: bool,
}

/// An `FsmString` value: a variable binding or a literal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum StrValue {
    Var(String),
    Literal(String),
}

/// An `FsmVar` value: a variable reference or a typed inline constant.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum VarValue {
    /// bound to a named variable
    Var(String),
    /// bound to a variable, unnamed
    Unset,
    Unused,
    #[serde(with = "super::float::f32_field")]
    Float(f32),
    Int(i32),
    Bool(bool),
    Str(String),
    Object(ObjectRef),
    #[serde(with = "super::float::f32_vec")]
    Vector(Vec<f32>),
    Enum(i32),
    Array(ArrayValue),
}

/// An `FsmEnum` value.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum EnumValue {
    Var(String),
    Named { enum_name: String, value: i32 },
    Value(i32),
}

/// An `FsmArray` value: a variable reference, or its elements.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum ArrayValue {
    Var(String),
    Values(Vec<Value>),
}

/// Any Fsm-wrapped parameter value: a variable binding or a typed literal (objects resolved).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum Value {
    Var(String),
    Bool(bool),
    Int(i32),
    #[serde(with = "super::float::f32_field")]
    Float(f32),
    Str(String),
    #[serde(with = "super::float::f32_vec")]
    Vector(Vec<f32>),
    Enum {
        enum_name: String,
        value: i32,
    },
    Object(ObjectRef),
    Array(ArrayValue),
}

/// A reflective method/property call (`FunctionCall`): the called member, its parameter type, and
/// the value of the active parameter (selected by `parameter_type`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Call<'a> {
    pub function: Cow<'a, str>,
    pub parameter_type: Cow<'a, str>,
    pub value: Option<Value>,
}

/// An animation curve (`FsmAnimationCurve`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Curve {
    pub keys: Vec<CurveKey>,
    pub pre_infinity: i32,
    pub post_infinity: i32,
    pub rotation_order: i32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CurveKey {
    #[serde(with = "super::float::f32_field")]
    pub time: f32,
    #[serde(with = "super::float::f32_field")]
    pub value: f32,
    #[serde(with = "super::float::f32_field")]
    pub in_slope: f32,
    #[serde(with = "super::float::f32_field")]
    pub out_slope: f32,
    #[serde(with = "super::float::f32_field")]
    pub in_weight: f32,
    #[serde(with = "super::float::f32_field")]
    pub out_weight: f32,
    pub weighted_mode: i32,
}

/// A template variable binding (`FsmVarOverride`): a template variable and the value bound to it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VarOverride {
    pub variable: String,
    pub value: VarValue,
}

/// Structured view of a whole FSM.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct FsmModel<'a> {
    pub name: Cow<'a, str>,
    /// `m_Name` of the `FsmTemplate` this FSM was built from, if any. The states
    /// and actions below are then the template's, and only the name and the
    /// inspector-exposed variables come from the component running it.
    pub template_name: Option<Cow<'a, str>>,
    pub start_state: Cow<'a, str>,
    pub events: Vec<Event<'a>>,
    pub global_transitions: Vec<Transition<'a>>,
    pub states: Vec<State<'a>>,
    pub variables: Vec<Variable<'a>>,
}

/// A declared FSM event.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Event<'a> {
    pub name: Cow<'a, str>,
    pub is_global: bool,
    pub is_system: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Transition<'a> {
    pub event: Cow<'a, str>,
    pub to_state: Cow<'a, str>,
}

/// The state's node rectangle in the PlayMaker editor graph (raw authored layout).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct StatePos {
    #[serde(with = "super::float::f32_field")]
    pub x: f32,
    #[serde(with = "super::float::f32_field")]
    pub y: f32,
    #[serde(with = "super::float::f32_field")]
    pub w: f32,
    #[serde(with = "super::float::f32_field")]
    pub h: f32,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct State<'a> {
    pub name: Cow<'a, str>,
    pub is_start: bool,
    /// Author-assigned PlayMaker colour group (0..=7 palette index; used only for display).
    pub color_index: u8,
    pub position: StatePos,
    pub transitions: Vec<Transition<'a>>,
    pub actions: Vec<Action<'a>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Action<'a> {
    /// Full action class name (e.g. `HutongGames.PlayMaker.Actions.SetBoolValue`).
    pub class: Cow<'a, str>,
    /// User-given label from the PlayMaker editor
    pub custom_name: Option<Cow<'a, str>>,
    pub enabled: bool,
    pub params: Vec<Param<'a>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Param<'a> {
    /// Field name; empty for unnamed params (e.g. array elements).
    pub name: Cow<'a, str>,
    /// PlayMaker parameter type name (see [`PARAM_TYPES`]).
    pub type_name: Cow<'a, str>,
    pub value: ParamValue<'a>,
}

/// A decoded parameter value.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum ParamValue<'a> {
    Bool(bool),
    Int(i32),
    #[serde(with = "super::float::f32_field")]
    Float(f32),
    /// Vector2/3, Quaternion, Color or Rect components.
    #[serde(with = "super::float::f32_vec")]
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
    Raw(Cow<'a, [u8]>),
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Variable<'a> {
    pub name: Cow<'a, str>,
    pub category: Cow<'a, str>,
    /// Exposed in the PlayMaker inspector, which is what makes it settable per
    /// instance when an FSM runs a template.
    pub show_in_inspector: bool,
    /// The variable's authored initial value (its FSM-editor default). At runtime actions may
    /// overwrite it; this is what the FSM ships with.
    pub value: Value,
}

/// A RunFSM template binding: the template to run plus how the parent FSM's variables and
/// events are wired into it. Variable bindings are either directional ([`inputs`](Self::inputs)
/// into the template, [`outputs`](Self::outputs) back out) or undirected
/// ([`overrides`](Self::overrides)); the unused lists are empty.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct TemplateControl<'a> {
    pub template: PathId,
    pub inputs: Vec<VarOverride>,
    pub outputs: Vec<VarOverride>,
    pub overrides: Vec<VarOverride>,
    pub events: Vec<(Cow<'a, str>, Cow<'a, str>)>,
}

#[cfg(test)]
mod tests {
    use rabex_env::component_path::{Component, ComponentId, ComponentPath, PathSegment};

    use super::*;

    fn component_path(path: &str, component: &str) -> ComponentPath {
        ComponentPath {
            segments: path
                .split('/')
                .map(|name| PathSegment {
                    name: name.to_string(),
                    index: None,
                })
                .collect(),
            component: Some(Component {
                id: ComponentId::Script(component.to_string()),
                index: None,
            }),
        }
    }

    /// A model covering every variant shape that carries borrowed data, so the
    /// round-trip below actually exercises them.
    fn sample() -> FsmModel<'static> {
        let object = |target| ObjectRef {
            file: Some("sharedassets9.assets".to_string()),
            target,
        };
        FsmModel {
            name: "Bench Control".into(),
            template_name: Some("bench_control".into()),
            start_state: "Init".into(),
            events: vec![Event {
                name: "FINISHED".into(),
                is_global: false,
                is_system: true,
            }],
            global_transitions: vec![Transition {
                event: "RESET".into(),
                to_state: "Init".into(),
            }],
            variables: vec![Variable {
                name: "Activated".into(),
                category: "bool".into(),
                show_in_inspector: true,
                value: Value::Bool(false),
            }],
            states: vec![State {
                name: "Init".into(),
                is_start: true,
                color_index: 0,
                position: StatePos {
                    x: 1.0,
                    y: 2.0,
                    w: 3.0,
                    h: 4.0,
                },
                transitions: vec![Transition {
                    event: "FINISHED".into(),
                    to_state: "Idle".into(),
                }],
                actions: vec![Action {
                    class: "HutongGames.PlayMaker.Actions.SetBoolValue".into(),
                    custom_name: Some("arm the bell".into()),
                    enabled: false,
                    params: vec![
                        Param {
                            name: "packed".into(),
                            type_name: "FsmBool".into(),
                            value: ParamValue::PackedVar(Some("Activated".to_string())),
                        },
                        Param {
                            name: "event".into(),
                            type_name: "FsmEvent".into(),
                            value: ParamValue::Event(None),
                        },
                        Param {
                            name: "str".into(),
                            type_name: "String".into(),
                            value: ParamValue::Str("hello".into()),
                        },
                        Param {
                            name: "fsmString".into(),
                            type_name: "FsmString".into(),
                            value: ParamValue::FsmString(StrValue::Var("Name".to_string())),
                        },
                        Param {
                            name: "owner".into(),
                            type_name: "FsmOwnerDefault".into(),
                            value: ParamValue::Owner(GoRef::SelfOwner),
                        },
                        Param {
                            name: "onPath".into(),
                            type_name: "FsmGameObject".into(),
                            value: ParamValue::GameObject(GoRef::Object(object(RefTarget::Path(
                                component_path("Whole Scene/Bell", "PlayMakerFSM"),
                            )))),
                        },
                        Param {
                            name: "loose".into(),
                            type_name: "FsmObject".into(),
                            value: ParamValue::Object(GoRef::Object(object(RefTarget::Loose {
                                name: Some("bell_hit".to_string()),
                                id: 4126,
                            }))),
                        },
                        Param {
                            name: "nullRef".into(),
                            type_name: "ObjectReference".into(),
                            value: ParamValue::Pptr(object(RefTarget::Null)),
                        },
                        Param {
                            name: "var".into(),
                            type_name: "FsmVar".into(),
                            value: ParamValue::Var(VarValue::Vector(vec![1.0, 2.0, 3.0])),
                        },
                        Param {
                            name: "target".into(),
                            type_name: "FsmEventTarget".into(),
                            value: ParamValue::EventTarget(EventTarget {
                                kind: 2,
                                game_object: GoRef::Var("Target".to_string()),
                                fsm_name: Some("Control".to_string()),
                                fsm: object(RefTarget::Null),
                                exclude_self: false,
                                send_to_children: true,
                            }),
                        },
                        Param {
                            name: "call".into(),
                            type_name: "FunctionCall".into(),
                            value: ParamValue::Function(Call {
                                function: "FadeToBlack".into(),
                                parameter_type: "float".into(),
                                value: Some(Value::Float(0.25)),
                            }),
                        },
                        Param {
                            name: "template".into(),
                            type_name: "FsmTemplateControl".into(),
                            value: ParamValue::Template(TemplateControl {
                                template: 77,
                                inputs: vec![VarOverride {
                                    variable: "In".to_string(),
                                    value: VarValue::Int(3),
                                }],
                                outputs: Vec::new(),
                                overrides: Vec::new(),
                                events: vec![("FROM".into(), "TO".into())],
                            }),
                        },
                        Param {
                            name: "enum".into(),
                            type_name: "FsmEnum".into(),
                            value: ParamValue::Enum(EnumValue::Named {
                                enum_name: "Ns.Type+Nested".to_string(),
                                value: 1,
                            }),
                        },
                        Param {
                            name: "member".into(),
                            type_name: "Enum".into(),
                            value: ParamValue::EnumMember("Multiply".into()),
                        },
                        Param {
                            name: "layer".into(),
                            type_name: "LayerMask".into(),
                            value: ParamValue::Layer {
                                index: 8,
                                name: Some("Terrain".into()),
                            },
                        },
                        Param {
                            name: "array".into(),
                            type_name: "FsmArray".into(),
                            value: ParamValue::Array(ArrayValue::Values(vec![Value::Int(1)])),
                        },
                        Param {
                            name: "property".into(),
                            type_name: "FsmProperty".into(),
                            value: ParamValue::Property(Property {
                                target: GoRef::SelfOwner,
                                type_name: "UnityEngine.Transform".into(),
                                property: "position".into(),
                                set: true,
                            }),
                        },
                        Param {
                            name: "curve".into(),
                            type_name: "FsmAnimationCurve".into(),
                            value: ParamValue::AnimCurve(Curve {
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
                        },
                        Param {
                            name: "list".into(),
                            type_name: "Array".into(),
                            value: ParamValue::List(vec![Param {
                                name: "".into(),
                                type_name: "Integer".into(),
                                value: ParamValue::Int(7),
                            }]),
                        },
                        Param {
                            name: "raw".into(),
                            type_name: "Unsupported".into(),
                            value: ParamValue::Raw(vec![1, 2, 3].into()),
                        },
                    ],
                }],
            }],
        }
    }

    /// The model is the interchange format: whatever we serialize has to read
    /// back into an equivalent model, so a consumer holding only the serialized
    /// form (the FSM index's content store) can render from it.
    #[test]
    fn model_round_trips_through_json() {
        let model = sample();
        let json = serde_json::to_string(&model).unwrap();
        let parsed: FsmModel = serde_json::from_str(&json).unwrap();
        assert_eq!(json, serde_json::to_string(&parsed).unwrap());
    }

    /// Curves in real data carry infinite tangents (stepped curves), and JSON
    /// cannot hold them — see [`super::super::float`]. They have to survive the
    /// round-trip rather than come back as `null`.
    #[test]
    fn non_finite_floats_survive_the_round_trip() {
        let curve = Curve {
            keys: vec![CurveKey {
                time: 0.5,
                value: f32::NAN,
                in_slope: f32::INFINITY,
                out_slope: f32::NEG_INFINITY,
                in_weight: 0.0,
                out_weight: 1.0,
                weighted_mode: 0,
            }],
            pre_infinity: 0,
            post_infinity: 0,
            rotation_order: 0,
        };
        let json = serde_json::to_string(&curve).unwrap();
        assert_eq!(
            json,
            r#"{"keys":[{"time":0.5,"value":"NaN","in_slope":"Infinity","out_slope":"-Infinity","in_weight":0.0,"out_weight":1.0,"weighted_mode":0}],"pre_infinity":0,"post_infinity":0,"rotation_order":0}"#
        );
        let parsed: Curve = serde_json::from_str(&json).unwrap();
        let key = &parsed.keys[0];
        assert!(key.value.is_nan());
        assert_eq!(key.in_slope, f32::INFINITY);
        assert_eq!(key.out_slope, f32::NEG_INFINITY);
        assert_eq!(key.time, 0.5);

        // A vector element is encoded the same way.
        let vector = Value::Vector(vec![1.0, f32::NAN]);
        let json = serde_json::to_string(&vector).unwrap();
        assert_eq!(json, r#"{"type":"Vector","value":[1.0,"NaN"]}"#);
        let Value::Vector(parsed) = serde_json::from_str(&json).unwrap() else {
            panic!("not a vector");
        };
        assert_eq!(parsed[0], 1.0);
        assert!(parsed[1].is_nan());
    }

    /// A `null` where a float belongs used to be how a non-finite value came
    /// back. It must fail loudly now rather than turn into NaN.
    #[test]
    fn null_is_not_a_float() {
        let err = serde_json::from_str::<Value>(r#"{"type":"Float","value":null}"#)
            .expect_err("null must not deserialize as a float");
        assert!(
            err.to_string().contains("invalid type: null"),
            "unexpected error: {err}"
        );
    }
}
