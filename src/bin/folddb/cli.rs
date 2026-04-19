use clap::{Parser, Subcommand};
use clap_complete::Shell;
use fold_db::schema::types::operations::MutationType;
use std::path::PathBuf;

/// FoldDB CLI - human-first command-line access to FoldDB
#[derive(Parser, Debug)]
#[command(name = "folddb", author, version, about)]
pub struct Cli {
    /// Output all results as JSON (for scripting)
    #[arg(long, global = true)]
    pub json: bool,

    /// Show verbose/debug output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Use dev environment (dev schema service + dev Exemem API)
    #[arg(long, global = true)]
    pub dev: bool,

    /// Path to node config file (also reads NODE_CONFIG env var)
    #[arg(long, global = true)]
    pub config: Option<String>,

    /// User hash for data isolation (also reads FOLD_USER_HASH env var)
    #[arg(long, global = true)]
    pub user_hash: Option<String>,

    /// Override: local Sled database directory
    #[arg(long, global = true)]
    pub data_path: Option<PathBuf>,

    /// Override: schema service URL
    #[arg(long, global = true)]
    pub schema_service_url: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage schemas
    Schema {
        #[command(subcommand)]
        action: SchemaCommand,
    },

    /// Execute a query against a schema
    Query {
        /// Schema name
        schema: String,

        /// Fields to query (comma-separated)
        #[arg(long)]
        fields: String,

        /// Filter hash key
        #[arg(long)]
        hash: Option<String>,

        /// Filter range key
        #[arg(long)]
        range: Option<String>,
    },

    /// Search the native word index
    Search {
        /// Search term
        term: String,
    },

    /// Execute mutations
    Mutate {
        #[command(subcommand)]
        action: MutateCommand,
    },

    /// Ingest data into FoldDB
    Ingest {
        #[command(subcommand)]
        action: IngestCommand,
    },

    /// Ask a natural-language question (LLM agent)
    Ask {
        /// The natural language query
        query: String,

        /// Maximum agent iterations (default: 10)
        #[arg(long, default_value = "10")]
        max_iterations: usize,
    },

    /// Show node status (key, user hash, config, indexing)
    Status,

    /// Show or inspect configuration
    Config {
        #[command(subcommand)]
        action: Option<ConfigCommand>,
    },

    /// Manage the background daemon
    Daemon {
        #[command(subcommand)]
        action: DaemonCommand,
    },

    /// Manage cloud backup sync
    Cloud {
        #[command(subcommand)]
        action: CloudCommand,
    },

    /// Backup/restore an encrypted snapshot of the local store via Exemem
    Snapshot {
        #[command(subcommand)]
        action: SnapshotCommand,
    },

    /// Manage organizations
    Org {
        #[command(subcommand)]
        action: OrgCommand,
    },

    /// Manage discovery network
    Discovery {
        #[command(subcommand)]
        action: DiscoveryCommand,
    },

    /// Display your 24-word recovery phrase
    RecoveryPhrase,

    /// Restore node from a 24-word recovery phrase
    Restore,

    /// Reset the database (destructive)
    Reset {
        /// Skip interactive confirmation
        #[arg(long)]
        confirm: bool,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
}

#[derive(Subcommand, Debug)]
pub enum SchemaCommand {
    /// List all schemas with their states
    List,
    /// Get a specific schema by name
    Get {
        /// Schema name
        name: String,
    },
    /// Approve a schema
    Approve {
        /// Schema name
        name: String,
    },
    /// Block a schema
    Block {
        /// Schema name
        name: String,
    },
    /// Load schemas from schema service
    Load,
}

#[derive(Subcommand, Debug)]
pub enum MutateCommand {
    /// Execute a single mutation
    Run {
        /// Schema name
        schema: String,

        /// Mutation type
        #[arg(long, value_enum)]
        r#type: MutationType,

        /// Fields as JSON object (e.g. '{"name":"value"}')
        #[arg(long)]
        fields: String,

        /// Hash key
        #[arg(long)]
        hash: Option<String>,

        /// Range key
        #[arg(long)]
        range: Option<String>,
    },
    /// Execute batch mutations from JSON file or stdin
    Batch {
        /// Path to JSON file (reads stdin if omitted)
        file: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum IngestCommand {
    /// Ingest JSON data from a file or stdin
    File {
        /// Path to JSON file (reads stdin if omitted)
        path: Option<PathBuf>,
    },
    /// Scan a folder using AI to classify files for ingestion
    SmartScan {
        /// Path to the folder to scan
        path: PathBuf,

        /// Maximum directory depth to scan (default: 5)
        #[arg(long, default_value = "5")]
        max_depth: usize,

        /// Maximum number of files to analyze (default: 500)
        #[arg(long, default_value = "500")]
        max_files: usize,
    },
    /// Ingest files from a folder using AI recommendations
    Smart {
        /// Base folder path
        path: PathBuf,

        /// First scan, then ingest all recommended files
        #[arg(long)]
        all: bool,

        /// Specific files to ingest (comma-separated, relative to folder)
        #[arg(long, value_delimiter = ',')]
        files: Option<Vec<String>>,

        /// Disable auto-execution of mutations
        #[arg(long)]
        no_execute: bool,
    },

    /// Ingest notes from Apple Notes (macOS only)
    #[cfg(target_os = "macos")]
    AppleNotes {
        /// Ingest only notes from this folder (default: all folders)
        #[arg(long)]
        folder: Option<String>,
        /// Batch size for ingestion (default: 10)
        #[arg(long, default_value = "10")]
        batch_size: usize,
    },

    /// Ingest photos from Apple Photos (macOS only)
    #[cfg(target_os = "macos")]
    ApplePhotos {
        /// Ingest only photos from this album
        #[arg(long)]
        album: Option<String>,
        /// Maximum number of photos to ingest (default: 50)
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Batch size (default: 5)
        #[arg(long, default_value = "5")]
        batch_size: usize,
    },

    /// Ingest reminders from Apple Reminders (macOS only)
    #[cfg(target_os = "macos")]
    AppleReminders {
        /// Ingest only reminders from this list (default: all lists)
        #[arg(long)]
        list: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Show current configuration
    Show,
    /// Print the config file path
    Path,
    /// Set a configuration value
    Set {
        /// Configuration key (e.g. "env")
        key: String,
        /// Configuration value (e.g. "dev" or "prod")
        value: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum DaemonCommand {
    /// Start the daemon in the background
    Start {
        /// Port to listen on (default: 9001)
        #[arg(long, default_value = "9001")]
        port: u16,
    },
    /// Stop the running daemon
    Stop,
    /// Show daemon status
    Status,
    /// Install as a system service (auto-start on login)
    Install,
    /// Uninstall the system service
    Uninstall,
}

#[derive(Subcommand, Debug)]
pub enum CloudCommand {
    /// Enable cloud backup (register with Exemem)
    Enable,
    /// Disable cloud backup (keep local data)
    Disable,
    /// Show cloud sync status
    Status,
    /// Trigger an immediate sync cycle
    Sync,
    /// Delete your Exemem account and all cloud data
    DeleteAccount,
}

#[derive(Subcommand, Debug)]
pub enum SnapshotCommand {
    /// Upload an encrypted snapshot of the current local store to Exemem
    Backup,
    /// Download the latest snapshot from Exemem and replay it into the local store
    Restore,
}

#[derive(Subcommand, Debug)]
pub enum OrgCommand {
    /// List organizations you belong to
    List,
    /// Create a new organization
    Create {
        /// Organization name
        name: String,
    },
    /// Show pending org invitations
    Invites,
    /// Join an org using an invite bundle (JSON from stdin or argument)
    Join {
        /// Invite bundle JSON (reads stdin if omitted)
        invite_json: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum DiscoveryCommand {
    /// Show discovery opt-ins and interests
    Status,
    /// Publish opted-in schemas to the discovery network
    Publish,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_schema_list() {
        let cli = Cli::parse_from(["folddb", "schema", "list"]);
        match cli.command {
            Command::Schema {
                action: SchemaCommand::List,
            } => {}
            _ => panic!("Expected Schema List"),
        }
    }

    #[test]
    fn parse_schema_get() {
        let cli = Cli::parse_from(["folddb", "schema", "get", "my_schema"]);
        match cli.command {
            Command::Schema {
                action: SchemaCommand::Get { name },
            } => assert_eq!(name, "my_schema"),
            _ => panic!("Expected Schema Get"),
        }
    }

    #[test]
    fn parse_schema_approve() {
        let cli = Cli::parse_from(["folddb", "schema", "approve", "tweets"]);
        match cli.command {
            Command::Schema {
                action: SchemaCommand::Approve { name },
            } => assert_eq!(name, "tweets"),
            _ => panic!("Expected Schema Approve"),
        }
    }

    #[test]
    fn parse_schema_block() {
        let cli = Cli::parse_from(["folddb", "schema", "block", "bad_schema"]);
        match cli.command {
            Command::Schema {
                action: SchemaCommand::Block { name },
            } => assert_eq!(name, "bad_schema"),
            _ => panic!("Expected Schema Block"),
        }
    }

    #[test]
    fn parse_schema_load() {
        let cli = Cli::parse_from(["folddb", "schema", "load"]);
        match cli.command {
            Command::Schema {
                action: SchemaCommand::Load,
            } => {}
            _ => panic!("Expected Schema Load"),
        }
    }

    #[test]
    fn parse_query() {
        let cli = Cli::parse_from([
            "folddb",
            "query",
            "tweets",
            "--fields",
            "text,author",
            "--hash",
            "abc",
        ]);
        match cli.command {
            Command::Query {
                schema,
                fields,
                hash,
                range,
            } => {
                assert_eq!(schema, "tweets");
                assert_eq!(fields, "text,author");
                assert_eq!(hash, Some("abc".to_string()));
                assert!(range.is_none());
            }
            _ => panic!("Expected Query"),
        }
    }

    #[test]
    fn parse_search() {
        let cli = Cli::parse_from(["folddb", "search", "hello world"]);
        match cli.command {
            Command::Search { term } => assert_eq!(term, "hello world"),
            _ => panic!("Expected Search"),
        }
    }

    #[test]
    fn parse_mutate_run() {
        let cli = Cli::parse_from([
            "folddb",
            "mutate",
            "run",
            "tweets",
            "--type",
            "create",
            "--fields",
            r#"{"text":"hello"}"#,
        ]);
        match cli.command {
            Command::Mutate {
                action:
                    MutateCommand::Run {
                        schema,
                        r#type,
                        fields,
                        ..
                    },
            } => {
                assert_eq!(schema, "tweets");
                assert_eq!(r#type, MutationType::Create);
                assert!(fields.contains("hello"));
            }
            _ => panic!("Expected Mutate Run"),
        }
    }

    #[test]
    fn parse_mutate_batch_with_file() {
        let cli = Cli::parse_from(["folddb", "mutate", "batch", "data.json"]);
        match cli.command {
            Command::Mutate {
                action: MutateCommand::Batch { file },
            } => assert_eq!(file, Some(PathBuf::from("data.json"))),
            _ => panic!("Expected Mutate Batch"),
        }
    }

    #[test]
    fn parse_mutate_batch_no_file() {
        let cli = Cli::parse_from(["folddb", "mutate", "batch"]);
        match cli.command {
            Command::Mutate {
                action: MutateCommand::Batch { file },
            } => assert!(file.is_none()),
            _ => panic!("Expected Mutate Batch"),
        }
    }

    #[test]
    fn parse_ingest_file() {
        let cli = Cli::parse_from(["folddb", "ingest", "file", "input.json"]);
        match cli.command {
            Command::Ingest {
                action: IngestCommand::File { path },
            } => assert_eq!(path, Some(PathBuf::from("input.json"))),
            _ => panic!("Expected Ingest File"),
        }
    }

    #[test]
    fn parse_ingest_file_no_path() {
        let cli = Cli::parse_from(["folddb", "ingest", "file"]);
        match cli.command {
            Command::Ingest {
                action: IngestCommand::File { path },
            } => assert!(path.is_none()),
            _ => panic!("Expected Ingest File"),
        }
    }

    #[test]
    fn parse_ingest_smart_scan() {
        let cli = Cli::parse_from(["folddb", "ingest", "smart-scan", "/tmp/data"]);
        match cli.command {
            Command::Ingest {
                action:
                    IngestCommand::SmartScan {
                        path,
                        max_depth,
                        max_files,
                    },
            } => {
                assert_eq!(path, PathBuf::from("/tmp/data"));
                assert_eq!(max_depth, 5);
                assert_eq!(max_files, 500);
            }
            _ => panic!("Expected Ingest SmartScan"),
        }
    }

    #[test]
    fn parse_ingest_smart_scan_with_options() {
        let cli = Cli::parse_from([
            "folddb",
            "ingest",
            "smart-scan",
            "/tmp/data",
            "--max-depth",
            "3",
            "--max-files",
            "100",
        ]);
        match cli.command {
            Command::Ingest {
                action:
                    IngestCommand::SmartScan {
                        max_depth,
                        max_files,
                        ..
                    },
            } => {
                assert_eq!(max_depth, 3);
                assert_eq!(max_files, 100);
            }
            _ => panic!("Expected Ingest SmartScan"),
        }
    }

    #[test]
    fn parse_ingest_smart_all() {
        let cli = Cli::parse_from(["folddb", "ingest", "smart", "/tmp/data", "--all"]);
        match cli.command {
            Command::Ingest {
                action:
                    IngestCommand::Smart {
                        path,
                        all,
                        no_execute,
                        files,
                    },
            } => {
                assert_eq!(path, PathBuf::from("/tmp/data"));
                assert!(all);
                assert!(!no_execute);
                assert!(files.is_none());
            }
            _ => panic!("Expected Ingest Smart"),
        }
    }

    #[test]
    fn parse_ingest_smart_with_files() {
        let cli = Cli::parse_from([
            "folddb",
            "ingest",
            "smart",
            "/tmp/data",
            "--files",
            "a.json,b.csv",
            "--no-execute",
        ]);
        match cli.command {
            Command::Ingest {
                action:
                    IngestCommand::Smart {
                        files,
                        no_execute,
                        all,
                        ..
                    },
            } => {
                assert_eq!(files, Some(vec!["a.json".to_string(), "b.csv".to_string()]));
                assert!(no_execute);
                assert!(!all);
            }
            _ => panic!("Expected Ingest Smart"),
        }
    }

    #[test]
    fn parse_ask() {
        let cli = Cli::parse_from(["folddb", "ask", "What is my data about?"]);
        match cli.command {
            Command::Ask {
                query,
                max_iterations,
            } => {
                assert_eq!(query, "What is my data about?");
                assert_eq!(max_iterations, 10);
            }
            _ => panic!("Expected Ask"),
        }
    }

    #[test]
    fn parse_ask_with_max_iterations() {
        let cli = Cli::parse_from(["folddb", "ask", "test", "--max-iterations", "5"]);
        match cli.command {
            Command::Ask { max_iterations, .. } => assert_eq!(max_iterations, 5),
            _ => panic!("Expected Ask"),
        }
    }

    #[test]
    fn parse_status() {
        let cli = Cli::parse_from(["folddb", "status"]);
        assert!(matches!(cli.command, Command::Status));
    }

    #[test]
    fn parse_config_show() {
        let cli = Cli::parse_from(["folddb", "config", "show"]);
        match cli.command {
            Command::Config {
                action: Some(ConfigCommand::Show),
            } => {}
            _ => panic!("Expected Config Show"),
        }
    }

    #[test]
    fn parse_config_path() {
        let cli = Cli::parse_from(["folddb", "config", "path"]);
        match cli.command {
            Command::Config {
                action: Some(ConfigCommand::Path),
            } => {}
            _ => panic!("Expected Config Path"),
        }
    }

    #[test]
    fn parse_config_bare() {
        let cli = Cli::parse_from(["folddb", "config"]);
        match cli.command {
            Command::Config { action: None } => {}
            _ => panic!("Expected Config with no subcommand"),
        }
    }

    #[test]
    fn parse_reset_with_confirm() {
        let cli = Cli::parse_from(["folddb", "reset", "--confirm"]);
        match cli.command {
            Command::Reset { confirm } => assert!(confirm),
            _ => panic!("Expected Reset"),
        }
    }

    #[test]
    fn parse_reset_without_confirm() {
        let cli = Cli::parse_from(["folddb", "reset"]);
        match cli.command {
            Command::Reset { confirm } => assert!(!confirm),
            _ => panic!("Expected Reset"),
        }
    }

    #[test]
    fn parse_cloud_sync() {
        let cli = Cli::parse_from(["folddb", "cloud", "sync"]);
        match cli.command {
            Command::Cloud {
                action: CloudCommand::Sync,
            } => {}
            _ => panic!("Expected Cloud Sync"),
        }
    }

    #[test]
    fn parse_cloud_status() {
        let cli = Cli::parse_from(["folddb", "cloud", "status"]);
        match cli.command {
            Command::Cloud {
                action: CloudCommand::Status,
            } => {}
            _ => panic!("Expected Cloud Status"),
        }
    }

    #[test]
    fn parse_snapshot_backup() {
        let cli = Cli::parse_from(["folddb", "snapshot", "backup"]);
        match cli.command {
            Command::Snapshot {
                action: SnapshotCommand::Backup,
            } => {}
            _ => panic!("Expected Snapshot Backup"),
        }
    }

    #[test]
    fn parse_snapshot_restore() {
        let cli = Cli::parse_from(["folddb", "snapshot", "restore"]);
        match cli.command {
            Command::Snapshot {
                action: SnapshotCommand::Restore,
            } => {}
            _ => panic!("Expected Snapshot Restore"),
        }
    }

    #[test]
    fn parse_completions() {
        let cli = Cli::parse_from(["folddb", "completions", "bash"]);
        match cli.command {
            Command::Completions { shell } => assert_eq!(shell, Shell::Bash),
            _ => panic!("Expected Completions"),
        }
    }

    #[test]
    fn parse_json_flag() {
        let cli = Cli::parse_from(["folddb", "--json", "status"]);
        assert!(cli.json);
        assert!(matches!(cli.command, Command::Status));
    }

    #[test]
    fn parse_verbose_flag() {
        let cli = Cli::parse_from(["folddb", "-v", "status"]);
        assert!(cli.verbose);
    }

    #[test]
    fn parse_data_path() {
        let cli = Cli::parse_from(["folddb", "--data-path", "/tmp/mydb", "status"]);
        assert_eq!(cli.data_path, Some(PathBuf::from("/tmp/mydb")));
    }

    #[test]
    fn parse_user_hash() {
        let cli = Cli::parse_from(["folddb", "--user-hash", "abc123", "status"]);
        assert_eq!(cli.user_hash, Some("abc123".to_string()));
    }

    #[test]
    fn parse_schema_service_url() {
        let cli = Cli::parse_from([
            "folddb",
            "--schema-service-url",
            "http://localhost:9002",
            "schema",
            "load",
        ]);
        assert_eq!(
            cli.schema_service_url,
            Some("http://localhost:9002".to_string())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parse_ingest_apple_notes_defaults() {
        let cli = Cli::parse_from(["folddb", "ingest", "apple-notes"]);
        match cli.command {
            Command::Ingest {
                action: IngestCommand::AppleNotes { folder, batch_size },
            } => {
                assert!(folder.is_none());
                assert_eq!(batch_size, 10);
            }
            _ => panic!("Expected Ingest AppleNotes"),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parse_ingest_apple_notes_with_options() {
        let cli = Cli::parse_from([
            "folddb",
            "ingest",
            "apple-notes",
            "--folder",
            "Work",
            "--batch-size",
            "25",
        ]);
        match cli.command {
            Command::Ingest {
                action: IngestCommand::AppleNotes { folder, batch_size },
            } => {
                assert_eq!(folder, Some("Work".to_string()));
                assert_eq!(batch_size, 25);
            }
            _ => panic!("Expected Ingest AppleNotes"),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parse_ingest_apple_photos_defaults() {
        let cli = Cli::parse_from(["folddb", "ingest", "apple-photos"]);
        match cli.command {
            Command::Ingest {
                action:
                    IngestCommand::ApplePhotos {
                        album,
                        limit,
                        batch_size,
                    },
            } => {
                assert!(album.is_none());
                assert_eq!(limit, 50);
                assert_eq!(batch_size, 5);
            }
            _ => panic!("Expected Ingest ApplePhotos"),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parse_ingest_apple_photos_with_options() {
        let cli = Cli::parse_from([
            "folddb",
            "ingest",
            "apple-photos",
            "--album",
            "Vacation",
            "--limit",
            "20",
            "--batch-size",
            "3",
        ]);
        match cli.command {
            Command::Ingest {
                action:
                    IngestCommand::ApplePhotos {
                        album,
                        limit,
                        batch_size,
                    },
            } => {
                assert_eq!(album, Some("Vacation".to_string()));
                assert_eq!(limit, 20);
                assert_eq!(batch_size, 3);
            }
            _ => panic!("Expected Ingest ApplePhotos"),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parse_ingest_apple_reminders_defaults() {
        let cli = Cli::parse_from(["folddb", "ingest", "apple-reminders"]);
        match cli.command {
            Command::Ingest {
                action: IngestCommand::AppleReminders { list },
            } => {
                assert!(list.is_none());
            }
            _ => panic!("Expected Ingest AppleReminders"),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parse_ingest_apple_reminders_with_list() {
        let cli = Cli::parse_from(["folddb", "ingest", "apple-reminders", "--list", "Shopping"]);
        match cli.command {
            Command::Ingest {
                action: IngestCommand::AppleReminders { list },
            } => {
                assert_eq!(list, Some("Shopping".to_string()));
            }
            _ => panic!("Expected Ingest AppleReminders"),
        }
    }

    #[test]
    fn parse_all_subcommands_exist() {
        let commands: Vec<Vec<&str>> = vec![
            vec!["folddb", "schema", "list"],
            vec!["folddb", "schema", "get", "x"],
            vec!["folddb", "schema", "approve", "x"],
            vec!["folddb", "schema", "block", "x"],
            vec!["folddb", "schema", "load"],
            vec!["folddb", "query", "s", "--fields", "f"],
            vec!["folddb", "search", "t"],
            vec![
                "folddb", "mutate", "run", "s", "--type", "create", "--fields", "{}",
            ],
            vec!["folddb", "mutate", "batch"],
            vec!["folddb", "ingest", "file"],
            vec!["folddb", "ingest", "smart-scan", "/tmp"],
            vec!["folddb", "ingest", "smart", "/tmp", "--all"],
            vec!["folddb", "ask", "test question"],
            vec!["folddb", "status"],
            vec!["folddb", "config"],
            vec!["folddb", "config", "show"],
            vec!["folddb", "config", "path"],
            vec!["folddb", "config", "set", "env", "dev"],
            vec!["folddb", "daemon", "start"],
            vec!["folddb", "daemon", "stop"],
            vec!["folddb", "daemon", "status"],
            vec!["folddb", "daemon", "install"],
            vec!["folddb", "daemon", "uninstall"],
            vec!["folddb", "cloud", "enable"],
            vec!["folddb", "cloud", "disable"],
            vec!["folddb", "cloud", "status"],
            vec!["folddb", "cloud", "sync"],
            vec!["folddb", "cloud", "delete-account"],
            vec!["folddb", "snapshot", "backup"],
            vec!["folddb", "snapshot", "restore"],
            vec!["folddb", "org", "list"],
            vec!["folddb", "org", "create", "TestOrg"],
            vec!["folddb", "org", "invites"],
            vec!["folddb", "org", "join", "{}"],
            vec!["folddb", "discovery", "status"],
            vec!["folddb", "discovery", "publish"],
            vec!["folddb", "recovery-phrase"],
            vec!["folddb", "restore"],
            vec!["folddb", "reset"],
            vec!["folddb", "completions", "bash"],
        ];

        #[cfg(target_os = "macos")]
        let commands = {
            let mut cmds = commands;
            cmds.push(vec!["folddb", "ingest", "apple-notes"]);
            cmds.push(vec!["folddb", "ingest", "apple-photos"]);
            cmds.push(vec!["folddb", "ingest", "apple-reminders"]);
            cmds
        };

        for args in &commands {
            let result = Cli::try_parse_from(args.iter());
            assert!(
                result.is_ok(),
                "Failed to parse: {:?}: {}",
                args,
                result.unwrap_err()
            );
        }
    }
}
