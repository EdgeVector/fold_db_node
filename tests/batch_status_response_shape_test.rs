//! Pin the JSON shape of `BatchStatusResponse` — the response body of
//! `GET /api/ingestion/batch/{batch_id}`.
//!
//! Three callers depend on these field names being stable:
//!
//! 1. The Rust struct in `src/ingestion/batch_controller.rs` (canonical).
//! 2. `BatchStatusResponse` in `src/server/static-react/src/types/openapi.ts`,
//!    auto-generated from the `ToSchema` derive on the struct above.
//! 3. The React `BatchStatusResponse` alias in
//!    `src/server/static-react/src/api/clients/ingestionClient.ts`, which
//!    re-exports the OpenAPI component.
//!
//! Add or rename a field on the Rust struct? Re-run `cargo run --bin
//! openapi_dump > target/openapi.json && npm --prefix
//! src/server/static-react run generate:api` and update this test.

use fold_db_node::ingestion::batch_controller::{
    BatchController, BatchStatusResponse, PendingFile,
};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Build a controller in a representative state (one completed, one failed,
/// one in flight, one still pending) so every field is exercised.
fn populated_controller() -> BatchController {
    let pending = vec![
        PendingFile {
            path: PathBuf::from("/tmp/a.txt"),
            progress_id: "p-a".into(),
            estimated_cost: 0.10,
        },
        PendingFile {
            path: PathBuf::from("/tmp/b.txt"),
            progress_id: "p-b".into(),
            estimated_cost: 0.20,
        },
        PendingFile {
            path: PathBuf::from("/tmp/c.txt"),
            progress_id: "p-c".into(),
            estimated_cost: 0.30,
        },
        PendingFile {
            path: PathBuf::from("/tmp/d.txt"),
            progress_id: "p-d".into(),
            estimated_cost: 0.40,
        },
    ];
    let mut ctrl = BatchController::new("batch-xyz".into(), Some(2.0), pending, false);
    let _ = ctrl.pop_next_file();
    ctrl.add_in_flight("a.txt".into(), "p-a".into());
    ctrl.record_completed("p-a", 0.12);

    let _ = ctrl.pop_next_file();
    ctrl.add_in_flight("b.txt".into(), "p-b".into());
    ctrl.record_failed("p-b", "b.txt".into(), "decode error".into());

    let _ = ctrl.pop_next_file();
    ctrl.add_in_flight("c.txt".into(), "p-c".into());

    ctrl
}

#[test]
fn batch_status_response_json_keys_are_canonical() {
    let ctrl = populated_controller();
    let resp = BatchStatusResponse::from_controller(&ctrl);
    let value = serde_json::to_value(&resp).expect("serialize");
    let object = value.as_object().expect("object");

    let actual: BTreeSet<&str> = object.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = [
        "batch_id",
        "status",
        "spend_limit",
        "accumulated_cost",
        "files_total",
        "files_completed",
        "files_failed",
        "failed_files",
        "files_remaining",
        "estimated_remaining_cost",
        "in_flight_count",
        "current_file_name",
        "current_file_step",
        "current_file_progress",
        "is_local_provider",
    ]
    .into_iter()
    .collect();

    assert_eq!(
        actual, expected,
        "BatchStatusResponse JSON keys drifted — frontend types in \
         openapi.ts and ingestionClient.ts will be out of sync. \
         Re-run the openapi.ts generator after intentional renames.",
    );
}

#[test]
fn batch_status_response_uses_canonical_status_values() {
    let ctrl = populated_controller();
    let resp = BatchStatusResponse::from_controller(&ctrl);
    let value = serde_json::to_value(&resp).expect("serialize");

    assert_eq!(
        value["status"], "Running",
        "BatchStatus serializes as PascalCase variants — see \
         BatchStatusResponse type alias in ingestionClient.ts.",
    );
}

#[test]
fn batch_status_response_field_values_match_controller_snapshot() {
    let ctrl = populated_controller();
    let resp = BatchStatusResponse::from_controller(&ctrl);
    let v = serde_json::to_value(&resp).expect("serialize");

    assert_eq!(v["batch_id"], "batch-xyz");
    assert_eq!(v["files_total"], 4);
    assert_eq!(v["files_completed"], 1);
    assert_eq!(v["files_failed"], 1);
    assert_eq!(v["files_remaining"], 1);
    assert_eq!(v["in_flight_count"], 1);
    assert_eq!(v["is_local_provider"], false);
    assert_eq!(v["current_file_name"], "c.txt");
    assert_eq!(v["failed_files"][0]["name"], "b.txt");
    assert_eq!(v["failed_files"][0]["error"], "decode error");
}
