use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::server::routes::schema::list_schemas,
        crate::server::routes::schema::load_schemas,
        crate::server::routes::schema::get_schema,
        crate::server::routes::schema::approve_schema,
        crate::server::routes::schema::block_schema,
        crate::server::routes::query::execute_query,
        crate::server::routes::query::execute_mutation,
        crate::server::routes::query::native_index_search,
        crate::server::routes::query::get_indexing_status,
        crate::server::routes::security::get_system_public_key,
        crate::server::routes::system::get_system_status,
        crate::server::routes::system::get_node_public_key,
        crate::server::routes::admin::reset_database,
        crate::server::routes::config::get_database_config,
        crate::server::routes::config::update_database_config,
        crate::server::routes::admin::migrate_to_cloud,
        crate::server::routes::log::list_logs,
        crate::server::routes::log::stream_logs,
        crate::server::routes::log::update_feature_level,
        crate::server::routes::ingestion::process_json,
        crate::server::routes::ingestion::get_status,
        crate::server::routes::ingestion::validate_json,
        crate::server::routes::ingestion::get_ingestion_config,
        crate::server::routes::ingestion::save_ingestion_config,
        crate::fold_node::llm_query::routes::chat
    ),
    components(
        schemas(
            // Schema/Field family registered as of Phase 4d' (2026-05-02): fold_db
            // Phase 3 slice 3 (#681) added ToSchema to FieldType/FieldAccessPolicy/
            // DataClassification/FieldValueType/AccessTier/CapabilityKind/
            // CapabilityConstraint and aliased FieldBase<M> for the four molecule
            // variants. Slice 3-follow-on (#682) added ToSchema to SchemaState/
            // SchemaWithState. Slice 3 also resolved the DeclarativeSchemaType
            // alias quirk (same class as slice 2's super::KeyMetadata fix in #679).
            //
            // The atom-module family was registered earlier (2026-05-02): fold_db
            // Phase 3 slice 1 (#678) added ToSchema to AtomEntry/KeyMetadata/
            // Provenance/MoleculeRef, and slice 2 (#679) fixed utoipa's
            // path-prefix `$ref` quirk on `super::KeyMetadata` field types.
            fold_db::schema::types::key_config::KeyConfig,
            fold_db::schema::types::key_value::KeyValue,
            fold_db::atom::AtomEntry,
            fold_db::atom::KeyMetadata,
            fold_db::atom::Provenance,
            fold_db::atom::MoleculeRef,
            fold_db::atom::Molecule,
            fold_db::atom::MoleculeHash,
            fold_db::atom::MoleculeRange,
            fold_db::atom::MoleculeHashRange,
            fold_db::schema::types::schema::DeclarativeSchemaType,
            fold_db::schema::types::declarative_schemas::DeclarativeSchemaDefinition,
            fold_db::schema::types::declarative_schemas::FieldDefinition,
            fold_db::schema::types::declarative_schemas::FieldMapper,
            fold_db::schema::types::declarative_schemas::SchemaSource,
            fold_db::schema::types::data_classification::DataClassification,
            fold_db::schema::types::field_value_type::FieldValueType,
            fold_db::schema::types::field::variant::FieldVariant,
            fold_db::schema::types::field::single_field::SingleField,
            fold_db::schema::types::field::range_field::RangeField,
            fold_db::schema::types::field::hash_field::HashField,
            fold_db::schema::types::field::hash_range_field::HashRangeField,
            fold_db::schema::types::field::common::FieldCommon,
            fold_db::schema::types::field::common::FieldType,
            fold_db::schema::types::transform::Transform,
            fold_db::schema::schema_types::SchemaState,
            fold_db::schema::schema_types::SchemaWithState,
            fold_db::access::types::FieldAccessPolicy,
            fold_db::access::types::AccessTier,
            fold_db::access::capability::CapabilityKind,
            fold_db::access::capability::CapabilityConstraint,
            crate::server::routes::schema::SchemaResponse,
            crate::ingestion::config::IngestionConfig,
            crate::ingestion::config::SavedConfig,
            crate::ingestion::config::AIProvider,
            crate::ingestion::config::OllamaConfig,
            crate::ingestion::config::OllamaGenerationParams,
            crate::ingestion::config::AnthropicConfig,
            crate::ingestion::config::VisionBackend,
            crate::ingestion::config::UseCaseOverride,
            crate::ingestion::roles::Role,
            crate::ingestion::IngestionRequest,
            crate::ingestion::IngestionResponse,
            crate::ingestion::IngestionStatus,
            crate::ingestion::progress::SchemaWriteRecord,
            crate::handlers::ingestion::ProcessJsonResponse,
            crate::server::routes::log::LogLevelUpdate,
            crate::server::routes::admin::ResetDatabaseRequest,
            crate::server::routes::admin::AdminJobResponse,
            crate::server::routes::config::DatabaseConfigRequest,
            crate::server::routes::config::DatabaseConfigResponse,
            crate::server::routes::config::DatabaseConfigDto,
            crate::server::routes::config::CloudSyncConfigDto,
            crate::server::routes::admin::MigrateToCloudRequest,
            crate::fold_node::llm_query::types::RunQueryRequest,
            crate::fold_node::llm_query::types::QueryPlan,
            crate::fold_node::llm_query::types::ChatRequest,
            crate::fold_node::llm_query::types::ChatResponse,
            fold_db::db_operations::IndexResult,
            fold_db::fold_db_core::orchestration::IndexingStatus,
            fold_db::fold_db_core::orchestration::IndexingState,
            crate::server::routes::query::MutationResponse,
            crate::handlers::system::NodeKeyResponse
        )
    ),
    tags(
        (name = "schemas", description = "Schema management endpoints"),
        (name = "query", description = "Query and mutation endpoints"),
        (name = "security", description = "Security and key management endpoints"),
        (name = "system", description = "System management endpoints"),
        (name = "logs", description = "Logging endpoints"),
        (name = "ingestion", description = "Ingestion endpoints"),
        (name = "llm-query", description = "LLM-powered natural language query endpoints")
    )
)]
struct ApiDoc;

pub fn build_openapi() -> String {
    serde_json::to_string(&ApiDoc::openapi())
        .expect("Failed to serialize OpenAPI documentation - this is a critical error")
}
