//! Resolve the names an FSM's own data cannot carry, by reading the game's
//! assemblies and its layer table.
//!
//! Two kinds of value arrive as bare integers:
//!
//! - **Enums.** A generic `Enum` param is a plain int whose type is implied by
//!   the action class plus field name. A typed `FsmEnum` param does carry its
//!   enum type's full name (`…Actions.HeroBoxControl+HeroBoxState`) but still
//!   only a number.
//! - **Layers.** PlayMaker tags layer fields with `[UIHint(UIHint.Layer)]`, so
//!   which params are layers comes from the assemblies, while the
//!   `index → name` table comes from the `TagManager`.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use anyhow::{Context as _, Result, anyhow};
use dotnetdll::prelude::*;
use dotnetdll::resolved::ResolvedDebug;
use dotnetdll::resolved::attribute::{FixedArg, IntegralParam};
use dotnetdll::resolved::members::{self, Constant};
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
    /// Every class name the assemblies define, so a bare action name resolves.
    classes: HashSet<String>,
    /// The fields of every action class, with the hash PlayMaker compares
    /// against `actionHashCodes` to decide how to load an action's params.
    actions: HashMap<String, ActionClass>,
    /// Layer name per index.
    layer_names: Vec<String>,
}

impl GameContext {
    /// `assemblies` hold the action classes and enums, named for error
    /// reporting. A class none of them defines keeps its enum and layer params
    /// numeric.
    pub fn new(
        playmaker_dll: &[u8],
        assemblies: &[(&str, &[u8])],
        layer_names: Vec<String>,
    ) -> Result<Self> {
        let mut context = GameContext {
            layer_names,
            ..Default::default()
        };

        let playmaker = parse(playmaker_dll).context("PlayMaker.dll")?;
        let others = assemblies
            .iter()
            .map(|(name, bytes)| parse(bytes).with_context(|| name.to_string()))
            .collect::<Result<Vec<_>>>()?;
        let all: Vec<&Resolution> = std::iter::once(&playmaker).chain(others.iter()).collect();

        let by_name = playmaker
            .enumerate_type_definitions()
            .map(|(idx, td)| (td.type_name(), idx))
            .collect();
        let resolver = UiHintResolver {
            res: &playmaker,
            by_name,
        };
        for res in &all {
            context.collect_enums(res);
            context.collect_layer_fields(res, &resolver);
        }
        context.classes = class_names(&all);
        context.actions = action_classes(&all);

        Ok(context)
    }

    /// Rewrite every param whose value these tables can name.
    pub fn apply(&self, model: &mut FsmModel<'_>) {
        for state in &mut model.states {
            for action in &mut state.actions {
                let class = self.class_name(action.class.as_ref()).to_string();
                if let Some(known) = self.actions.get(&class) {
                    known.load(action);
                }
                for param in &mut action.params {
                    let class = action.class.as_ref();
                    self.bake_enum(class, param);
                    self.bake_layer(class, param);
                }
            }
        }
    }

    /// The stored hash of an action class as the assemblies define it today, so
    /// a tool can report how many actions no longer match what was saved.
    pub fn action_type_hash(&self, class: &str) -> Option<i32> {
        self.actions
            .get(self.class_name(class).as_ref())
            .map(|known| known.hash)
    }

    /// The name the assemblies know an action by. `actionNames` may hold a bare
    /// class name, which PlayMaker resolves against its own action namespace.
    fn class_name<'n>(&self, class: &'n str) -> Cow<'n, str> {
        if self.classes.contains(class) {
            return class.into();
        }
        let qualified = format!("HutongGames.PlayMaker.Actions.{class}");
        match self.classes.contains(&qualified) {
            true => qualified.into(),
            false => class.into(),
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

/// An action class as the assemblies define it today.
struct ActionClass {
    /// Public instance fields, inherited ones included, in reflection order.
    fields: Vec<Field>,
    /// `GetActionTypeHashCode`: the stable hash over the field types.
    hash: i32,
}

struct Field {
    name: String,
    /// The `ParamDataType` this field's type serialises as, as `FindField`
    /// compares it.
    param_type: &'static str,
}

impl ActionClass {
    /// Assign the params the way `ActionData.CreateAction` does: positionally
    /// while the stored hash matches this class, and by name and param type
    /// through `FindField` once it does not.
    fn load(&self, action: &mut crate::model::Action<'_>) {
        if action.type_hash == self.hash {
            action.params.truncate(self.fields.len());
            for (param, field) in action.params.iter_mut().zip(&self.fields) {
                param.name = field.name.clone().into();
            }
            return;
        }
        let mut left: Vec<Option<crate::model::Param<'_>>> =
            action.params.drain(..).map(Some).collect();
        let mut recovered = Vec::with_capacity(self.fields.len());
        for field in &self.fields {
            let found = left.iter().position(|slot| {
                slot.as_ref().is_some_and(|param| {
                    param.name == field.name.as_str() && param.type_name == field.param_type
                })
            });
            if let Some(at) = found {
                recovered.push(left[at].take().expect("position found a filled slot"));
            }
        }
        action.params = recovered;
    }
}

/// The action classes of these assemblies, with their field types hashed the way
/// `GetActionTypeHashCode` does. A class whose base chain leaves the assemblies
/// we read is left out: its field list would be short, and a short list drops
/// params the action does have.
fn action_classes(resolutions: &[&Resolution]) -> HashMap<String, ActionClass> {
    /// Where every inheritance chain ends, in an assembly we do not read.
    const OBJECT: &str = "System.Object";

    struct Own {
        fields: Vec<(String, String)>,
        base: Option<String>,
    }
    let mut own: HashMap<String, Own> = HashMap::new();
    for res in resolutions {
        for (_idx, td) in res.enumerate_type_definitions() {
            let fields = td
                .fields
                .iter()
                .filter(|field| {
                    !field.static_member
                        && field.accessibility
                            == members::Accessibility::Access(Accessibility::Public)
                })
                .map(|field| {
                    (
                        field.name.to_string(),
                        type_to_string(res, &field.return_type),
                    )
                })
                .collect();
            let base = td.extends.as_ref().map(|source| match source {
                TypeSource::User(user) => user_type_name(res, user),
                TypeSource::Generic { base, .. } => user_type_name(res, base),
            });
            own.entry(nested_name(res, td))
                .or_insert(Own { fields, base });
        }
    }

    own.keys()
        .filter_map(|name| {
            let mut fields: Vec<(String, String)> = Vec::new();
            let mut at = name.as_str();
            loop {
                let class = own.get(at)?;
                fields.extend(class.fields.iter().cloned());
                match class.base.as_deref() {
                    None | Some(OBJECT) => break,
                    Some(base) => at = base,
                }
            }
            let hash = stable_hash(
                &fields
                    .iter()
                    .map(|(_, ty)| format!("{ty}|"))
                    .collect::<String>(),
            );
            let fields = fields
                .into_iter()
                .map(|(name, ty)| Field {
                    name,
                    param_type: param_data_type(&ty),
                })
                .collect();
            Some((name.clone(), ActionClass { fields, hash }))
        })
        .collect()
}

/// `Type.ToString()` as .NET renders it, which is what the hash is taken over.
fn type_to_string(res: &Resolution, ty: &MemberType) -> String {
    let base = match ty {
        MemberType::Base(base) => &**base,
        MemberType::TypeGeneric(index) => return format!("!{index}"),
    };
    match base {
        BaseType::Type { source, .. } => match source {
            TypeSource::User(user) => user_type_name(res, user),
            TypeSource::Generic { base, parameters } => format!(
                "{}[{}]",
                user_type_name(res, base),
                parameters
                    .iter()
                    .map(|p| type_to_string(res, p))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        },
        BaseType::Boolean => "System.Boolean".into(),
        BaseType::Char => "System.Char".into(),
        BaseType::Int8 => "System.SByte".into(),
        BaseType::UInt8 => "System.Byte".into(),
        BaseType::Int16 => "System.Int16".into(),
        BaseType::UInt16 => "System.UInt16".into(),
        BaseType::Int32 => "System.Int32".into(),
        BaseType::UInt32 => "System.UInt32".into(),
        BaseType::Int64 => "System.Int64".into(),
        BaseType::UInt64 => "System.UInt64".into(),
        BaseType::Float32 => "System.Single".into(),
        BaseType::Float64 => "System.Double".into(),
        BaseType::IntPtr => "System.IntPtr".into(),
        BaseType::UIntPtr => "System.UIntPtr".into(),
        BaseType::Object => "System.Object".into(),
        BaseType::String => "System.String".into(),
        BaseType::Vector(_, inner) => format!("{}[]", type_to_string(res, inner)),
        BaseType::Array(inner, shape) => format!(
            "{}[{}]",
            type_to_string(res, inner),
            ",".repeat(shape.rank.saturating_sub(1))
        ),
        other => format!("{other:?}"),
    }
}

/// The `ParamDataType` a field of this type serialises as. Only the mapping
/// `FindField` compares on; anything else it would call `CustomClass`.
fn param_data_type(field_type: &str) -> &'static str {
    let unqualified = field_type.rsplit('.').next().unwrap_or(field_type);
    match unqualified {
        "Boolean" => "Boolean",
        "Int32" => "Integer",
        "Single" => "Float",
        "String" => "String",
        "Color" => "Color",
        "Vector2" => "Vector2",
        "Vector3" => "Vector3",
        "Vector4" => "Vector4",
        "Quaternion" => "Quaternion",
        "Rect" => "Rect",
        _ => crate::model::PARAM_TYPES
            .iter()
            .copied()
            .find(|name| *name == unqualified)
            .unwrap_or("CustomClass"),
    }
}

/// `GetStableHash`: Jenkins one-at-a-time over the UTF-16 bytes, truncated.
fn stable_hash(text: &str) -> i32 {
    let mut hash: u32 = 0;
    for byte in text.encode_utf16().flat_map(u16::to_le_bytes) {
        hash = hash.wrapping_add(byte as u32);
        hash = hash.wrapping_add(hash << 10);
        hash ^= hash >> 6;
    }
    hash = hash.wrapping_add(hash << 3);
    hash ^= hash >> 11;
    hash = hash.wrapping_add(hash << 15);
    (hash % 100_000_000) as i32
}

/// The name of a type as [`nested_name`] spells it, defined here or referenced.
fn user_type_name(res: &Resolution, user: &UserType) -> String {
    match user {
        UserType::Definition(idx) => nested_name(res, &res[*idx]),
        UserType::Reference(idx) => {
            let type_ref = &res[*idx];
            match type_ref.scope {
                ResolutionScope::Nested(encloser) => format!(
                    "{}+{}",
                    user_type_name(res, &UserType::Reference(encloser)),
                    type_ref.name
                ),
                _ => type_ref.type_name(),
            }
        }
    }
}

/// Every class name the assemblies define, spelled the way `actionNames` does.
fn class_names(resolutions: &[&Resolution]) -> HashSet<String> {
    resolutions
        .iter()
        .flat_map(|res| {
            res.enumerate_type_definitions()
                .map(move |(_idx, td)| nested_name(res, td))
        })
        .collect()
}

/// The name `actionNames` and `MonoScript` spell a type with: `Outer+Inner` for
/// a nested type, namespace-qualified otherwise.
fn nested_name(res: &Resolution, td: &dotnetdll::resolved::types::TypeDefinition) -> String {
    match td.encloser {
        Some(encloser) => format!("{}+{}", nested_name(res, &res[encloser]), td.name),
        None => td.type_name(),
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
