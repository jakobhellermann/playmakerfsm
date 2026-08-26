//! Decode a raw [`Fsm`] into the structured [`super`] model types, resolving object pointers.
//!
//! ActionData stores all params of a state flat across `paramName/paramDataType/paramDataPos`,
//! sliced per action by `actionStartIndex` (no trailing sentinel). Reference params (FsmString,
//! FsmOwnerDefault, FsmVar, …) live in a typed array that `paramDataPos` indexes; value primitives
//! (Boolean/Integer/Float and the packed Fsm-wrappers/FsmEvent) live in flat `byteData` at
//! `paramDataPos` (`paramByteDataSize` = length), packed as `[value(n)][useVariable(1)][name(rest)]`.

use std::collections::HashMap;

use super::types::*;
use crate::raw::*;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::qualify::Qualifier;
use rabex_env::rabex::objects::PPtr;
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
            exclude_self: t.excludeSelf.value != 0,
            send_to_children: t.sendToChildren.value != 0,
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
            13 => VarValue::Array(self.array_value(&v.arrayValue)),
            14 => VarValue::Enum(v.intValue),
            3 | 9 | 10 | 12 => VarValue::Object(self.resolve(v.objectReference)),
            5 | 6 | 7 | 8 | 11 => {
                let w = &v.vector4Value;
                VarValue::Vector(vec![w.x, w.y, w.z, w.w])
            }
            t => unreachable!("unknown VariableType ordinal: {t}"),
        }
    }

    fn array_value(&mut self, a: &FsmArray) -> ArrayValue {
        if a.useVariable != 0 && !a.name.is_empty() {
            return ArrayValue::Var(a.name.clone());
        }
        // a PlayMaker array is homogeneous; its element type selects which value vec is populated
        let values = match a.r#type {
            0 => a.floatValues.iter().map(|&f| Value::Float(f)).collect(),
            1 => a.intValues.iter().map(|&i| Value::Int(i)).collect(),
            2 => a.boolValues.iter().map(|&b| Value::Bool(b != 0)).collect(),
            4 => a
                .stringValues
                .iter()
                .map(|s| Value::Str(s.clone()))
                .collect(),
            3 | 9 | 10 | 12 => a
                .objectReferences
                .iter()
                .map(|&p| Value::Object(self.resolve(p)))
                .collect(),
            5 | 6 | 7 | 8 | 11 => a
                .vector4Values
                .iter()
                .map(|v| Value::Vector(vec![v.x, v.y, v.z, v.w]))
                .collect(),
            _ => Vec::new(),
        };
        ArrayValue::Values(values)
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
                .map(|e| {
                    (
                        e.fromEvent.name.as_str().into(),
                        e.toEvent.name.as_str().into(),
                    )
                })
                .collect(),
        }
    }

    fn value_object(&mut self, use_var: u8, name: &str, value: PPtr) -> Value {
        if use_var != 0 {
            Value::Var(name.to_owned())
        } else {
            Value::Object(self.resolve(value))
        }
    }

    /// The value of a [`FunctionCall`]'s active parameter, selected by its `parameterType`.
    fn fn_param(&mut self, f: &FunctionCall) -> Option<Value> {
        // PlayMaker stores `parameterType` as the C# type name: keyword-aliased and lowercase for
        // primitives (`bool`, `int`, `float`, `string`, `object`), the bare type name for the rest
        // (`Vector2`, `GameObject`, `Enum`, …). Match case-insensitively so neither casing slips
        // through to the catch-all (which is now a hard error).
        Some(match f.parameterType.to_ascii_lowercase().as_str() {
            "none" | "" => return None,
            "bool" => value_bool(&f.BoolParameter),
            "int" => value_int(&f.IntParameter),
            "float" => value_float(&f.FloatParameter),
            "string" => value_string(&f.StringParameter),
            "vector2" => value_vec(
                f.Vector2Parameter.useVariable,
                &f.Vector2Parameter.name,
                vec![f.Vector2Parameter.value.x, f.Vector2Parameter.value.y],
            ),
            "vector3" => value_vec(
                f.Vector3Parameter.useVariable,
                &f.Vector3Parameter.name,
                vec![
                    f.Vector3Parameter.value.x,
                    f.Vector3Parameter.value.y,
                    f.Vector3Parameter.value.z,
                ],
            ),
            "quaternion" => value_vec(
                f.QuaternionParameter.useVariable,
                &f.QuaternionParameter.name,
                vec![
                    f.QuaternionParameter.value.x,
                    f.QuaternionParameter.value.y,
                    f.QuaternionParameter.value.z,
                    f.QuaternionParameter.value.w,
                ],
            ),
            "color" => value_vec(
                f.ColorParameter.useVariable,
                &f.ColorParameter.name,
                vec![
                    f.ColorParameter.value.r,
                    f.ColorParameter.value.g,
                    f.ColorParameter.value.b,
                    f.ColorParameter.value.a,
                ],
            ),
            "rect" => value_vec(
                f.RectParamater.useVariable,
                &f.RectParamater.name,
                vec![
                    f.RectParamater.value.x,
                    f.RectParamater.value.y,
                    f.RectParamater.value.width,
                    f.RectParamater.value.height,
                ],
            ),
            "gameobject" => self.value_object(
                f.GameObjectParameter.useVariable,
                &f.GameObjectParameter.name,
                f.GameObjectParameter.value,
            ),
            "object" => self.value_object(
                f.ObjectParameter.useVariable,
                &f.ObjectParameter.name,
                f.ObjectParameter.value,
            ),
            "material" => self.value_object(
                f.MaterialParameter.useVariable,
                &f.MaterialParameter.name,
                f.MaterialParameter.value,
            ),
            "texture" => self.value_object(
                f.TextureParameter.useVariable,
                &f.TextureParameter.name,
                f.TextureParameter.value,
            ),
            "enum" => value_enum(&f.EnumParameter),
            "array" => Value::Array(self.array_value(&f.ArrayParameter)),
            other => unreachable!("unknown FunctionCall parameterType: {other:?}"),
        })
    }
}

fn value_bool(f: &FsmBool) -> Value {
    if f.useVariable != 0 {
        Value::Var(f.name.clone())
    } else {
        Value::Bool(f.value != 0)
    }
}
fn value_int(f: &FsmInt) -> Value {
    if f.useVariable != 0 {
        Value::Var(f.name.clone())
    } else {
        Value::Int(f.value)
    }
}
fn value_float(f: &FsmFloat) -> Value {
    if f.useVariable != 0 {
        Value::Var(f.name.clone())
    } else {
        Value::Float(f.value)
    }
}
fn value_string(s: &FsmString) -> Value {
    if s.useVariable != 0 && !s.name.is_empty() {
        Value::Var(s.name.clone())
    } else {
        Value::Str(s.value.clone())
    }
}
fn value_vec(use_var: u8, name: &str, components: Vec<f32>) -> Value {
    if use_var != 0 && !name.is_empty() {
        Value::Var(name.to_owned())
    } else {
        Value::Vector(components)
    }
}
fn value_enum(e: &FsmEnum) -> Value {
    if e.useVariable != 0 && !e.name.is_empty() {
        Value::Var(e.name.clone())
    } else {
        Value::Enum {
            enum_name: e.enumName.clone(),
            value: e.intValue,
        }
    }
}

/// Resolve a [`Fsm`] into the structured [`FsmModel`], resolving object pointers via `ctx`.
pub fn decode_fsm<'a, R: EnvResolver, P: TypeTreeProvider>(
    fsm: &'a Fsm,
    ctx: &mut Context<'_, R, P>,
) -> FsmModel<'a> {
    FsmModel {
        name: fsm.name.as_str().into(),
        // only a component carries the enabled flag
        enabled: true,
        // only a component knows whether it runs a template
        template_name: None,
        start_state: fsm.startState.as_str().into(),
        events: fsm
            .events
            .iter()
            .map(|e| Event {
                name: e.name.as_str().into(),
                is_global: e.isGlobal != 0,
                is_system: e.isSystemEvent != 0,
            })
            .collect(),
        global_transitions: fsm.globalTransitions.iter().map(transition).collect(),
        states: fsm
            .states
            .iter()
            .map(|s| decode_state(s, &fsm.startState, fsm.dataVersion, &mut *ctx))
            .collect(),
        variables: decode_variables(&fsm.variables, &mut *ctx),
    }
}

fn transition(t: &FsmTransition) -> Transition<'_> {
    Transition {
        event: t.fsmEvent.name.as_str().into(),
        to_state: t.toState.as_str().into(),
    }
}

fn decode_state<'a, R: EnvResolver, P: TypeTreeProvider>(
    s: &'a FsmState,
    start: &str,
    version: i32,
    ctx: &mut Context<'_, R, P>,
) -> State<'a> {
    let ad = &s.actionData;
    let actions = ad
        .actionNames
        .iter()
        .enumerate()
        .map(|(ai, cls)| Action {
            class: cls.as_str().into(),
            // `ActionData` stores `~AutoName` for an action the editor names
            // after its class (`autoNameString`), so that is the absence of a
            // custom name rather than one.
            custom_name: ad
                .customNames
                .get(ai)
                .filter(|c| !c.is_empty() && *c != "~AutoName")
                .map(|c| c.as_str().into()),
            enabled: ad.actionEnabled.get(ai) != Some(&0),
            params: decode_params(ad, ai, version, &mut *ctx),
        })
        .collect();
    State {
        name: s.name.as_str().into(),
        is_start: s.name == start,
        is_sequence: s.isSequence != 0,
        color_index: s.colorIndex,
        position: StatePos {
            x: s.position.x,
            y: s.position.y,
            w: s.position.width,
            h: s.position.height,
        },
        transitions: s.transitions.iter().map(transition).collect(),
        actions,
    }
}

/// Decode action `ai`'s parameter slice into typed [`Param`]s.
fn decode_params<'a, R: EnvResolver, P: TypeTreeProvider>(
    ad: &'a ActionData,
    ai: usize,
    version: i32,
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

    let mut out = Vec::new();
    let mut j = lo as usize;
    while j < hi {
        let before = j;
        match decode_one(ad, &mut j, hi, version, ctx) {
            Some(p) => out.push(p),
            None => break,
        }
        if j == before {
            break;
        }
    }
    out
}

/// Decode the param at `*j`, advancing `*j` past it. An inline `Array` param consumes the `N`
/// following entries as its elements, a `CustomClass` param the `N` following entries as its
/// fields.
fn decode_one<'a, R: EnvResolver, P: TypeTreeProvider>(
    ad: &'a ActionData,
    j: &mut usize,
    hi: usize,
    version: i32,
    ctx: &mut Context<'_, R, P>,
) -> Option<Param<'a>> {
    let dt = *ad.paramDataType.get(*j)?;
    let pos = *ad.paramDataPos.get(*j)? as usize;
    let size = ad.paramByteDataSize.get(*j).copied().unwrap_or(0) as usize;
    let type_name = ptype(dt);
    let name = ad.paramName.get(*j).map(String::as_str).unwrap_or("");
    *j += 1;

    let value = if type_name == "Array" {
        let n = ad.arrayParamSizes.get(pos).copied().unwrap_or(0).max(0) as usize;
        let mut elems = Vec::with_capacity(n);
        for _ in 0..n {
            if *j >= hi {
                break;
            }
            if let Some(child) = decode_one(ad, j, hi, version, ctx) {
                elems.push(child);
            }
        }
        ParamValue::List(elems)
    } else if type_name == "CustomClass" {
        // `paramDataPos` indexes the custom-type tables rather than a value
        // array: the size says how many of the following params are its fields,
        // and those can be nested classes themselves.
        // Indexed rather than probed: a miss would take zero fields and read this
        // class's own fields as its siblings, shifting every param after it.
        let n = ad.customTypeSizes[pos].max(0) as usize;
        let class = ad.customTypeNames[pos].as_str();
        let mut fields = Vec::with_capacity(n);
        for _ in 0..n {
            assert!(
                *j < hi,
                "class {class:?} declares {n} fields, which run past the action's params"
            );
            if let Some(child) = decode_one(ad, j, hi, version, ctx) {
                fields.push(child);
            }
        }
        ParamValue::Class {
            class: class.into(),
            fields,
        }
    } else {
        decode_param(ad, type_name, pos, size, version, ctx)
    };
    Some(Param {
        name: name.into(),
        type_name: type_name.into(),
        value,
    })
}

fn decode_param<'a, R: EnvResolver, P: TypeTreeProvider>(
    ad: &'a ActionData,
    type_name: &str,
    pos: usize,
    size: usize,
    version: i32,
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
        "String" if version > 1 => ad
            .stringParams
            .get(pos)
            .map(|s| ParamValue::Str(Cow::Borrowed(s))),
        // dataVersion 1 packs strings into byteData, where no bytes is the empty string
        "String" => Some(ParamValue::Str(Cow::Borrowed(""))),
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
                function: f.FunctionName.as_str().into(),
                parameter_type: f.parameterType.as_str().into(),
                value: ctx.fn_param(f),
            })
        }),
        "FsmTemplateControl" => ad
            .fsmTemplateControlParams
            .get(pos)
            .map(|t| ParamValue::Template(ctx.template_control(t))),
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
                type_name: p.TargetTypeName.as_str().into(),
                property: p.PropertyName.as_str().into(),
                set: p.setProperty != 0,
            })
        }),
        "FsmAnimationCurve" => ad
            .animationCurveParams
            .get(pos)
            .map(|c| ParamValue::AnimCurve(curve(c))),
        // FsmEvent: in dataVersion 1 the event name is packed into byteData; from version 2 on it
        // lives in `stringParams[pos]` (an empty entry means no event), like the raw `String` type.
        "FsmEvent" => Some(ParamValue::Event(if version > 1 {
            ad.stringParams
                .get(pos)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        } else if size == 0 {
            None
        } else {
            longest_ascii_run(byte_slice(bd, pos, size))
        })),
        _ => None,
    };
    decoded.unwrap_or_else(|| ParamValue::Raw(byte_slice(bd, pos, size).into()))
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
                in_weight: k.inWeight,
                out_weight: k.outWeight,
                weighted_mode: k.weightedMode,
            })
            .collect(),
        pre_infinity: c.curve.m_PreInfinity,
        post_infinity: c.curve.m_PostInfinity,
        rotation_order: c.curve.m_RotationOrder,
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
        return ParamValue::Raw(byte_slice(bd, pos, size).into());
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
        return ParamValue::Raw(byte_slice(bd, pos, size).into());
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

pub fn decode_variables<'a, R: EnvResolver, P: TypeTreeProvider>(
    v: &'a FsmVariables,
    ctx: &mut Context<'_, R, P>,
) -> Vec<Variable<'a>> {
    let mut out: Vec<Variable> = Vec::new();
    // `FsmVariables.AddVariableLookup` keys every variable by name in one
    // dictionary and keeps the first of a repeated name, so a later duplicate
    // cannot be read or written by anything. Nameless ones never enter the
    // lookup at all. Both are skipped here, or the model would describe
    // variables the FSM has no way to address.
    //
    // Every repeat seen so far carries the same value, so which one survives is
    // not a decision. One that differs would mean this reading is incomplete.
    let mut addressable: HashMap<&str, usize> = HashMap::new();
    // each variable's `value` is its authored default; `useVariable`/`name` (the reference machinery
    // the param helpers honour) is irrelevant on a variable definition, so read the value directly.
    macro_rules! push {
        ($field:ident, $label:literal, |$x:ident| $value:expr) => {
            for $x in &v.$field {
                if $x.name.is_empty() {
                    continue;
                }
                let value = $value;
                match addressable.get(&$x.name.as_str()) {
                    Some(&first) => {
                        let kept = &out[first];
                        assert!(
                            same_value(&kept.value, &value),
                            "variable {:?} is repeated with a different value: {} {:?} vs {} {:?}",
                            $x.name,
                            kept.category,
                            kept.value,
                            $label,
                            value,
                        );
                    }
                    None => {
                        addressable.insert($x.name.as_str(), out.len());
                        out.push(Variable {
                            name: $x.name.as_str().into(),
                            category: $label.into(),
                            show_in_inspector: $x.showInInspector != 0,
                            value,
                        });
                    }
                }
            }
        };
    }
    push!(floatVariables, "float", |x| Value::Float(x.value));
    push!(intVariables, "int", |x| Value::Int(x.value));
    push!(boolVariables, "bool", |x| Value::Bool(x.value != 0));
    push!(stringVariables, "string", |x| Value::Str(x.value.clone()));
    push!(vector2Variables, "vector2", |x| Value::Vector(vec![
        x.value.x, x.value.y
    ]));
    push!(vector3Variables, "vector3", |x| Value::Vector(vec![
        x.value.x, x.value.y, x.value.z
    ]));
    push!(colorVariables, "color", |x| Value::Vector(vec![
        x.value.r, x.value.g, x.value.b, x.value.a
    ]));
    push!(rectVariables, "rect", |x| Value::Vector(vec![
        x.value.x,
        x.value.y,
        x.value.width,
        x.value.height
    ]));
    push!(quaternionVariables, "quaternion", |x| Value::Vector(vec![
        x.value.x, x.value.y, x.value.z, x.value.w
    ]));
    push!(gameObjectVariables, "gameObject", |x| Value::Object(
        ctx.resolve(x.value)
    ));
    push!(objectVariables, "object", |x| Value::Object(
        ctx.resolve(x.value)
    ));
    push!(materialVariables, "material", |x| Value::Object(
        ctx.resolve(x.value)
    ));
    push!(textureVariables, "texture", |x| Value::Object(
        ctx.resolve(x.value)
    ));
    push!(arrayVariables, "array", |x| Value::Array(
        ctx.array_value(x)
    ));
    push!(enumVariables, "enum", |x| Value::Enum {
        enum_name: x.enumName.clone(),
        value: x.intValue,
    });
    out
}

/// Compare two authored values through their serialized form, so a NaN — which
/// is never equal to itself as an `f32` — does not read as a difference.
fn same_value(a: &Value, b: &Value) -> bool {
    let render = |v| serde_json::to_string(v).expect("an authored value serializes");
    render(a) == render(b)
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

/// PlayMaker's `FsmVariables.OverrideVariableValues`: an FSM built from a
/// template keeps the template's variables, but every variable the template
/// exposes in the inspector takes its value from the instance.
///
/// `variables` are the template's, decoded; `instance` is the raw
/// `FsmVariables` of the component running it.
pub fn override_inspector_values<'a, R: EnvResolver, P: TypeTreeProvider>(
    variables: &mut [Variable<'a>],
    instance: &'a FsmVariables,
    ctx: &mut Context<'_, R, P>,
) {
    if !variables.iter().any(|v| v.show_in_inspector) {
        return;
    }
    let overrides = decode_variables(instance, ctx);
    for variable in variables {
        if !variable.show_in_inspector {
            continue;
        }
        if let Some(instance_value) = overrides
            .iter()
            .find(|o| o.category == variable.category && o.name == variable.name)
        {
            variable.value = instance_value.value.clone();
        }
    }
}
