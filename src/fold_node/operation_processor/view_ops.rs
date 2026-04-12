use fold_db::error::FoldDbResult;
use fold_db::view::registry::ViewState;
pub use fold_db::view::types::TransformView;

use super::OperationProcessor;

impl OperationProcessor {
    /// List all views with their states.
    pub async fn list_views(&self) -> FoldDbResult<Vec<(TransformView, ViewState)>> {
        let db = self.get_db()?;
        Ok(db.schema_manager().get_views_with_states()?)
    }

    /// Get a specific view by name.
    pub async fn get_view(&self, name: &str) -> FoldDbResult<Option<TransformView>> {
        let db = self.get_db()?;
        Ok(db.schema_manager().get_view(name)?)
    }

    /// Register a new transform view.
    pub async fn create_view(&self, view: TransformView) -> FoldDbResult<()> {
        let db = self.get_db()?;
        Ok(db.schema_manager().register_view(view).await?)
    }

    /// Approve a view for queries and mutations.
    pub async fn approve_view(&self, name: &str) -> FoldDbResult<()> {
        let db = self.get_db()?;
        Ok(db.schema_manager().approve_view(name).await?)
    }

    /// Block a view from queries and mutations.
    pub async fn block_view(&self, name: &str) -> FoldDbResult<()> {
        let db = self.get_db()?;
        Ok(db.schema_manager().block_view(name).await?)
    }

    /// Delete (remove) a view and clean up storage.
    pub async fn delete_view(&self, name: &str) -> FoldDbResult<()> {
        let db = self.get_db()?;
        Ok(db.schema_manager().remove_view(name).await?)
    }

    /// Load a view from the global schema service, including all transitive
    /// dependencies (source schemas and source views).
    pub async fn load_view(
        &self,
        name: &str,
    ) -> FoldDbResult<crate::fold_node::node::ViewLoadResult> {
        self.node.load_view_from_service(name).await
    }
}
