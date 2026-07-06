pub mod workspace_mapper;

use std::sync::Arc;

use super::SkillManagerHandle;

pub fn register_default_skills(handle: &SkillManagerHandle) {
    handle.register(Arc::new(workspace_mapper::WorkspaceMapper));
}
