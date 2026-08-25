//! Read a `PlayMakerFSM` component and decode the FSM it actually runs.
//!
//! A component either carries its own FSM or points at an [`FsmTemplate`]. In
//! the latter case its own `fsm` data is dead: `PlayMakerFSM.InitTemplate()`
//! builds a fresh FSM from the template and keeps only a few fields of the
//! component's.

use anyhow::Result;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;

use crate::model::{Context, FsmModel, decode_fsm, override_inspector_values};
use crate::raw::{FsmTemplate, PlayMakerFSM};

/// `MonoScript::full_name()` of the component this module reads.
pub const SCRIPT_NAME: &str = "PlayMakerFSM";

/// A `PlayMakerFSM` component together with the template it runs, if any.
pub struct ComponentFsm {
    component: PlayMakerFSM,
    template: Option<FsmTemplate>,
}

impl ComponentFsm {
    /// Read the component at `path_id`, following its template pointer.
    ///
    /// Fails when the object doesn't deserialize as a `PlayMakerFSM` — the raw
    /// layout is versioned per PlayMaker release, so a game whose shape
    /// [`crate::raw`] doesn't cover ends up here.
    pub fn read<R: EnvResolver, P: TypeTreeProvider>(
        file: &SerializedFileHandle<'_, R, P>,
        path_id: PathId,
    ) -> Result<Self> {
        let component = file.object_at::<PlayMakerFSM>(path_id)?.read()?;
        let template = match component.fsmTemplate.m_PathID {
            0 => None,
            _ => Some(file.deref(component.fsmTemplate)?.read()?),
        };
        Ok(ComponentFsm {
            component,
            template,
        })
    }

    /// Decode the FSM that runs at runtime, resolving object pointers.
    ///
    /// `file` must be the serialized file the component was read from.
    pub fn decode<R: EnvResolver, P: TypeTreeProvider>(
        &self,
        file: &SerializedFileHandle<'_, R, P>,
    ) -> Result<FsmModel<'_>> {
        let Some(template) = &self.template else {
            return Ok(decode_fsm(&self.component.fsm, &mut Context::new(file)));
        };
        // The template lives in its own serialized file, so its action pointers
        // index *that* file's external table. Resolving them against the
        // component's file lands cross-bundle references in the wrong bundle.
        let template_file = file.deref(self.component.fsmTemplate)?;
        let mut model = decode_fsm(&template.fsm, &mut Context::new(&template_file.file));
        model.name = self.component.fsm.name.as_str().into();
        model.template_name = Some(template.m_Name.as_str().into());
        // The component's variable values reach the template FSM, and its object
        // references point into the component's file, not the template's.
        override_inspector_values(
            &mut model.variables,
            &self.component.fsm.variables,
            &mut Context::new(file),
        );
        Ok(model)
    }
}
