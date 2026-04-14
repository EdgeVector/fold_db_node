use clap::Parser;
use fold_db::constants::DEFAULT_SCHEMA_SERVICE_PORT;
use fold_db_node::schema_service::SchemaServiceServer;

/// Command line options for the schema service binary.
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Cli {
    /// Port for the schema service
    #[arg(long, default_value_t = DEFAULT_SCHEMA_SERVICE_PORT)]
    port: u16,

    /// Path to the sled database for storing schemas
    #[arg(long, default_value = "schema_registry")]
    db_path: String,
}

/// Main entry point for the Schema Service.
///
/// This service provides HTTP endpoints for schema discovery and retrieval.
/// Uses local Sled storage for schema persistence.
///
/// # Command-Line Arguments
///
/// * `--port <PORT>` - Port for the schema service (default: 9002)
/// * `--db-path <PATH>` - Path to the sled database (default: schema_registry)
///
/// # Errors
///
/// Returns an error if:
/// * The database cannot be opened
/// * The HTTP server cannot be started
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    fold_db::logging::LoggingSystem::init_default().await.ok();

    // Parse command-line arguments
    let Cli { port, db_path } = Cli::parse();

    let bind_address = format!("127.0.0.1:{}", port);

    let server = {
        println!("🚀 Schema service starting with local sled storage");
        println!("   Database path: {}", db_path);

        SchemaServiceServer::new_with_builtins(db_path, &bind_address).await?
    };

    println!("✅ Schema service listening on {}", bind_address);

    server
        .run()
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::Parser;
    use fold_db::constants::DEFAULT_SCHEMA_SERVICE_PORT;

    #[test]
    fn defaults() {
        let cli = Cli::parse_from(["test"]);
        assert_eq!(cli.port, DEFAULT_SCHEMA_SERVICE_PORT);
        assert_eq!(cli.db_path, "schema_registry");
    }

    #[test]
    fn custom_args() {
        let cli = Cli::parse_from(["test", "--port", "8000", "--db-path", "my_schema_db"]);
        assert_eq!(cli.port, 8000);
        assert_eq!(cli.db_path, "my_schema_db");
    }
}
