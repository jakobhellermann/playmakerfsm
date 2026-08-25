//! Resolve the names an FSM's own data cannot carry, by reading the game's
//! assemblies and its layer table.
//!
//! Two kinds of value arrive as bare integers:
//!
//! - **Enums.** A generic `Enum` param is a plain int whose type is implied by
//!   the action class plus field name. A typed `FsmEnum` param does carry its
//!   enum type's full name (`…Actions.HeroBoxControl+HeroBoxState`) but still
//!   only a number. Both resolve against the enum definitions in
//!   `Assembly-CSharp.dll`.
//! - **Layers.** PlayMaker tags layer fields with `[UIHint(UIHint.Layer)]`, so
//!   which params are layers comes from the assemblies, while the
//!   `index → name` table comes from the `TagManager`.
//!
//! Both assemblies are required: failing is better than rendering every enum
//! as a number and calling it a result.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use anyhow::{Context as _, Result, anyhow};
use dotnetdll::prelude::*;
use dotnetdll::resolved::ResolvedDebug;
use dotnetdll::resolved::attribute::{FixedArg, IntegralParam};
use dotnetdll::resolved::members::Constant;
use dotnetdll::resolved::types::{BaseType, MemberType, Resolver, TypeSource, UserType};
use rabex_env::Environment;
use rabex_env::rabex::objects::{ClassId, ClassIdType};
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;
use serde::Deserialize;

use crate::model::{EnumValue, FsmModel, ParamValue};

/// `{value → member}` for one enum type.
type Members = HashMap<i32, String>;

/// What a game knows that its FSM data does not: enum members and layer
/// names.
#[derive(Default)]
pub struct GameContext {
    /// `(action class, field)` → members, for generic `Enum` params.
    enums_by_field: HashMap<(String, String), Members>,
    /// Full enum type name → members, for typed `FsmEnum` params.
    enums_by_name: HashMap<String, Members>,
    /// `(action class, field)` of every `[UIHint(UIHint.Layer)]` field.
    layer_fields: HashSet<(String, String)>,
    /// Layer name per index.
    layer_names: Vec<String>,
}

impl GameContext {
    pub fn new(
        playmaker_dll: &[u8],
        assembly_csharp: &[u8],
        layer_names: Vec<String>,
    ) -> Result<Self> {
        let mut context = GameContext {
            layer_names,
            ..Default::default()
        };

        let assembly_csharp = parse(assembly_csharp).context("Assembly-CSharp.dll")?;
        context.collect_enums(&assembly_csharp);

        // The `UIHint` enum lives in PlayMaker.dll, so attribute blobs in both
        // assemblies are decoded against it.
        let playmaker = parse(playmaker_dll).context("PlayMaker.dll")?;
        let by_name = playmaker
            .enumerate_type_definitions()
            .map(|(idx, td)| (td.type_name(), idx))
            .collect();
        let resolver = UiHintResolver {
            res: &playmaker,
            by_name,
        };
        context.collect_layer_fields(&playmaker, &resolver);
        context.collect_layer_fields(&assembly_csharp, &resolver);

        Ok(context)
    }

    /// Rewrite every param whose value these tables can name.
    pub fn apply(&self, model: &mut FsmModel<'_>) {
        for state in &mut model.states {
            for action in &mut state.actions {
                for param in &mut action.params {
                    let class = action.class.as_ref();
                    self.bake_enum(class, param);
                    self.bake_layer(class, param);
                }
            }
        }
    }

    fn bake_enum(&self, class: &str, param: &mut crate::model::Param<'_>) {
        let resolved = match &param.value {
            ParamValue::Int(v) if param.type_name == "Enum" => {
                let key = (class.to_string(), param.name.to_string());
                self.enums_by_field.get(&key).and_then(|m| m.get(v))
            }
            ParamValue::Enum(EnumValue::Named { enum_name, value }) => {
                self.enums_by_name.get(enum_name).and_then(|m| m.get(value))
            }
            _ => None,
        };
        if let Some(name) = resolved {
            param.value = ParamValue::EnumMember(Cow::Owned(name.clone()));
        }
    }

    fn bake_layer(&self, class: &str, param: &mut crate::model::Param<'_>) {
        if !self
            .layer_fields
            .contains(&(class.to_string(), param.name.to_string()))
        {
            return;
        }
        let to_layer = |index: i32| ParamValue::Layer {
            index,
            name: self.layer_name(index).map(|n| Cow::Owned(n.to_owned())),
        };
        match &mut param.value {
            ParamValue::Int(i) => param.value = to_layer(*i),
            // `layerMask`-style int arrays
            ParamValue::List(elems) => {
                for elem in elems {
                    if let ParamValue::Int(i) = elem.value {
                        elem.value = to_layer(i);
                    }
                }
            }
            _ => {}
        }
    }

    fn layer_name(&self, index: i32) -> Option<&str> {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.layer_names.get(i))
            .filter(|name| !name.is_empty())
            .map(String::as_str)
    }

    fn collect_enums(&mut self, res: &Resolution) {
        for (_idx, td) in res.enumerate_type_definitions() {
            if let Some(members) = members_of(td) {
                self.enums_by_name
                    .insert(full_type_name(res, td), members.clone());
            }
            let class = match &td.namespace {
                Some(ns) if !ns.is_empty() => format!("{ns}.{}", td.name),
                _ => td.name.to_string(),
            };
            for field in &td.fields {
                if let Some(members) = enum_members(res, &field.return_type) {
                    self.enums_by_field
                        .insert((class.clone(), field.name.to_string()), members);
                }
            }
        }
    }

    fn collect_layer_fields(&mut self, res: &Resolution, resolver: &UiHintResolver) {
        /// `HutongGames.PlayMaker.UIHint.Layer`.
        const UI_HINT_LAYER: i32 = 8;

        for (_idx, td) in res.enumerate_type_definitions() {
            let class = td.type_name();
            for field in &td.fields {
                for attr in &field.attributes {
                    // UIHint is the only attribute on a PlayMaker field whose
                    // constructor takes a single int-backed enum.
                    if !attr.constructor.show(res).contains("UIHint") {
                        continue;
                    }
                    let Ok(data) = attr.instantiation_data(resolver, res) else {
                        continue;
                    };
                    if let Some(FixedArg::Integral(IntegralParam::Int32(UI_HINT_LAYER))) =
                        data.constructor_args.first()
                    {
                        self.layer_fields
                            .insert((class.clone(), field.name.to_string()));
                    }
                }
            }
        }
    }
}

fn parse(bytes: &[u8]) -> Result<Resolution<'_>> {
    Resolution::parse(bytes, ReadOptions::default()).map_err(|e| anyhow!("{e}"))
}

/// `{value → member}` for an enum type definition, or `None` if `td` isn't one.
fn members_of(td: &dotnetdll::resolved::types::TypeDefinition) -> Option<Members> {
    // every enum has a synthetic `value__` field holding the underlying value
    if !td.fields.iter().any(|f| f.name == "value__") {
        return None;
    }
    let members: Members = td
        .fields
        .iter()
        .filter(|f| f.literal)
        .filter_map(|f| match &f.default {
            Some(Constant::Int32(v)) => Some((*v, f.name.to_string())),
            _ => None,
        })
        .collect();
    (!members.is_empty()).then_some(members)
}

/// If `ty` is a same-assembly (Definition) enum, its members, else `None`.
fn enum_members(res: &Resolution, ty: &MemberType) -> Option<Members> {
    let MemberType::Base(b) = ty else {
        return None;
    };
    let BaseType::Type {
        source: TypeSource::User(UserType::Definition(idx)),
        ..
    } = b.as_ref()
    else {
        return None;
    };
    members_of(&res[*idx])
}

/// A type name in PlayMaker's `enumName` form: `Namespace.Outer+Nested`, one
/// `+` per nesting level. Matches the form `Action::class` carries.
pub fn full_type_name(res: &Resolution, td: &dotnetdll::resolved::types::TypeDefinition) -> String {
    match td.encloser {
        Some(enc) => format!("{}+{}", full_type_name(res, &res[enc]), td.name),
        None => match &td.namespace {
            Some(ns) if !ns.is_empty() => format!("{ns}.{}", td.name),
            _ => td.name.to_string(),
        },
    }
}

/// Resolves type names against PlayMaker.dll, so an attribute blob carrying a
/// `UIHint` value can be decoded from an assembly that doesn't define it.
struct UiHintResolver<'a> {
    res: &'a Resolution<'a>,
    by_name: HashMap<String, TypeIndex>,
}

#[derive(Debug)]
struct TypeNotFound(String);

impl std::fmt::Display for TypeNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "type not found: {}", self.0)
    }
}

impl std::error::Error for TypeNotFound {}

impl<'a> Resolver<'a> for UiHintResolver<'a> {
    type Error = TypeNotFound;

    fn find_type(
        &self,
        name: &str,
    ) -> Result<
        (
            &dotnetdll::resolved::types::TypeDefinition<'a>,
            &Resolution<'a>,
        ),
        Self::Error,
    > {
        let idx = *self
            .by_name
            .get(name)
            .ok_or_else(|| TypeNotFound(name.to_string()))?;
        Ok((&self.res[idx], self.res))
    }
}

#[derive(Deserialize)]
struct TagManager {
    layers: Vec<String>,
}

impl ClassIdType for TagManager {
    const CLASS_ID: ClassId = ClassId::TagManager;
}

/// `index → layer name` from the `TagManager` in `globalgamemanagers`. Unnamed
/// slots come back as empty strings so an index still lines up with its slot.
pub fn layer_names<R: EnvResolver, P: TypeTreeProvider>(
    env: &Environment<R, P>,
) -> Result<Vec<String>> {
    let tag_manager = env
        .globalgamemanagers()
        .and_then(|ggm| ggm.find_object_of::<TagManager>())?
        .context("globalgamemanagers has no TagManager")?;
    Ok(tag_manager.layers)
}
