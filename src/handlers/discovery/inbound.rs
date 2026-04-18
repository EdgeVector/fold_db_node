//! Discovery inbound message pipeline.
//!
//! This module owns the request-polling + decrypted-message dispatch path:
//! `poll_and_decrypt_requests` and every `handle_incoming_*` / `process_*`
//! helper fed by `dispatch_decrypted_message`. Outbound handlers
//! (`opt_in`, `publish`, `search`, `connect`, `respond_to_request`, etc.)
//! live in `mod.rs`.
//!
//! Keeping the inbound pipeline in its own file keeps `mod.rs` readable and
//! isolates the non-trivial dedup / retry invariants covered by the tests
//! at the bottom of this file.

use super::get_metadata_store;
use super::util::{
    collect_our_pseudonyms, encode_marker_timestamp, now_secs, prune_msg_processed_markers,
    DEDUP_RETENTION_SECS, MSG_PROCESSED_PREFIX, PRUNE_EVERY_N_POLLS, PRUNE_POLL_COUNTER,
};
use super::ConnectionRequestsResponse;
use crate::discovery::async_query::{
    self, IdentityCardMessagePayload, QueryRequestPayload, QueryResponsePayload, SchemaInfo,
    SchemaListRequestPayload, SchemaListResponsePayload,
};
use crate::discovery::connection::{
    self, ConnectionPayload, DataSharePayload, LocalConnectionRequest, MutualContact,
    ReferralQueryPayload, ReferralResponsePayload, ShareInvitePayload, Vouch,
};
#[cfg(test)]
use crate::discovery::connection::{SharedRecord, SharedRecordKey};
use crate::discovery::publisher::DiscoveryPublisher;
use crate::discovery::received_card::{self, LocalReceivedCard};
use crate::fold_node::node::FoldNode;
use crate::fold_node::OperationProcessor;
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult, IntoHandlerError};
use crate::trust::contact_book::{Contact, ContactBook, TrustDirection};
use crate::trust::sharing_roles::SharingRoleConfig;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use std::collections::HashMap;

pub async fn poll_and_decrypt_requests(
    node: &FoldNode,
    discovery_url: &str,
    auth_token: &str,
    master_key: &[u8],
) -> HandlerResult<ConnectionRequestsResponse> {
    let publisher = DiscoveryPublisher::new(
        master_key.to_vec(),
        discovery_url.to_string(),
        auth_token.to_string(),
    );

    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    // Get our published pseudonyms via the shared helper — same derivation as the
    // publisher uses when uploading: derive(master_key, SHA256(embedding_bytes)).
    let our_pseudonyms: Vec<uuid::Uuid> = collect_our_pseudonyms(node, master_key).await?;

    if our_pseudonyms.is_empty() {
        return Ok(ApiResponse::success(ConnectionRequestsResponse {
            requests: Vec::new(),
        }));
    }

    // Poll messages in chunks of ≤100 pseudonyms. The messaging service requires a
    // pseudonym filter (returns empty if None), so we must always send a filter.
    // For large pseudonym sets, split into multiple polls and merge results.
    const POLL_CHUNK_SIZE: usize = 100;
    let mut messages = Vec::new();
    for chunk in our_pseudonyms.chunks(POLL_CHUNK_SIZE) {
        let chunk_messages = publisher
            .poll_messages(None, Some(chunk))
            .await
            .handler_err("poll messages")?;
        messages.extend(chunk_messages);
    }
    log::info!(
        "Polled {} pseudonyms in {} chunks, got {} messages",
        our_pseudonyms.len(),
        our_pseudonyms.len().div_ceil(POLL_CHUNK_SIZE),
        messages.len()
    );

    // Try to decrypt and dispatch each message. The loop enforces the invariant
    // that a message is marked "processed" in the dedup store ONLY after the
    // dispatch handler reports either successful handling (`Handled`) or a
    // permanent parse/unknown failure (`Skipped`). Transient errors (store
    // hiccups, failed persistence) propagate as `Err` and leave the dedup marker
    // absent, so the next poll retries the same message.
    //
    // CLAUDE.md "no silent failures": we no longer swallow dispatch errors with
    // `log::warn!; continue;`. Errors are collected and logged at the end, and
    // the dedup store write itself is checked (no `let _ = ...`).
    let mut dispatch_errors: Vec<(String, HandlerError)> = Vec::new();
    for msg in &messages {
        let target: uuid::Uuid = match msg.target_pseudonym.parse() {
            Ok(u) => u,
            // Not addressable — cannot possibly be for us. Skip without dedup
            // marker (cheap to re-skip next poll; avoids polluting the store).
            Err(_) => continue,
        };

        // Derive the secret key for this pseudonym
        let (secret, _) = connection::derive_pseudonym_keypair(master_key, &target);

        let encrypted_bytes = match B64.decode(&msg.encrypted_blob) {
            Ok(b) => b,
            Err(_) => continue,
        };

        let raw = match connection::decrypt_message_raw(&secret, &encrypted_bytes) {
            Ok(v) => v,
            Err(e) => {
                log::debug!(
                    "Failed to decrypt message {} for target {}: {}",
                    msg.message_id,
                    target,
                    e
                );
                // Not for us, corrupted, or from a different sender key. We
                // intentionally do NOT mark this as processed: decrypt cost is
                // low and the bulletin board expires messages on its own.
                continue;
            }
        };

        // De-duplication: check if we already processed this message.
        // Any present value counts as "processed"; the stored value is a
        // wall-clock timestamp (little-endian u64 seconds) used only by the
        // periodic prune of stale markers.
        let dedup_key = format!("{}{}", MSG_PROCESSED_PREFIX, msg.message_id);
        match store.get(dedup_key.as_bytes()).await {
            Ok(Some(_)) => continue,
            Ok(None) => {}
            Err(e) => {
                // Store read failure — treat as transient, do not dispatch.
                dispatch_errors.push((
                    msg.message_id.clone(),
                    HandlerError::Internal(format!("dedup read failed: {e}")),
                ));
                continue;
            }
        }

        // Dispatch. The helper returns Handled/Skipped to indicate the message
        // should be marked processed, or Err to indicate a transient failure
        // that must be retried on the next poll.
        let outcome =
            dispatch_decrypted_message(node, &*store, master_key, &publisher, msg, raw).await;

        match outcome {
            Ok(DispatchOutcome::Handled) | Ok(DispatchOutcome::Skipped { .. }) => {
                if let Ok(DispatchOutcome::Skipped { ref reason }) = outcome {
                    log::warn!(
                        "Permanently skipping message {}: {}",
                        msg.message_id,
                        reason
                    );
                }
                // Only mark as processed after the handler reports a terminal
                // outcome. Propagate errors from the dedup write — a silent
                // failure here would cause the message to re-dispatch forever.
                // Store the write timestamp so the periodic prune can expire
                // stale markers (see G2 prune module above).
                if let Err(e) = store
                    .put(dedup_key.as_bytes(), encode_marker_timestamp(now_secs()))
                    .await
                {
                    dispatch_errors.push((
                        msg.message_id.clone(),
                        HandlerError::Internal(format!("dedup write failed: {e}")),
                    ));
                }
            }
            Err(e) => {
                log::error!(
                    "Transient dispatch failure for message {} (will retry next poll): {}",
                    msg.message_id,
                    e
                );
                dispatch_errors.push((msg.message_id.clone(), e));
            }
        }
    }

    if !dispatch_errors.is_empty() {
        log::error!(
            "poll_and_decrypt_requests: {} message(s) failed dispatch and will be retried on the next poll",
            dispatch_errors.len()
        );
    }

    // Prune stale dedup markers every N-th poll. Done after the dispatch loop
    // so we never delete a marker we're about to check in the same iteration.
    let prune_count = PRUNE_POLL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if prune_count.is_multiple_of(PRUNE_EVERY_N_POLLS) {
        match prune_msg_processed_markers(&*store, now_secs(), DEDUP_RETENTION_SECS).await {
            Ok(deleted) => {
                if deleted > 0 {
                    log::info!("Pruned {} stale msg_processed markers", deleted);
                }
            }
            Err(e) => log::error!("Failed to prune msg_processed markers: {}", e),
        }
    }

    // Return all locally stored received requests
    let requests = connection::list_received_requests(&*store)
        .await
        .handler_err("list received requests")?;

    Ok(ApiResponse::success(ConnectionRequestsResponse {
        requests,
    }))
}

/// Outcome of dispatching a single decrypted bulletin-board message.
///
/// Both `Handled` and `Skipped` cause the caller to write the dedup marker.
/// `Err(HandlerError)` (returned separately) means the dispatch should be
/// retried on the next poll and the dedup marker must NOT be written.
#[derive(Debug)]
enum DispatchOutcome {
    /// Dispatch succeeded.
    Handled,
    /// Message is permanently unprocessable (bad/unknown payload). The caller
    /// writes the dedup marker so we don't re-dispatch garbage on every poll.
    Skipped { reason: String },
}

/// Dispatch a single decrypted message to the appropriate type-specific
/// handler. This replaces the prior "log warn and continue" pattern that
/// silently lost data when a transient error occurred between marking a
/// message processed and actually persisting it.
///
/// Classification:
/// - Transient (returns `Err`, caller retries): `save_received_request`,
///   `update_sent_request_status`, `process_accepted_connection`,
///   `process_data_share`. These can fail on a transient Sled hiccup or a
///   missing-but-recoverable prerequisite, and must be retried.
/// - Permanent (returns `Ok(Skipped)`, caller marks processed): payload
///   deserialization failure, unknown `message_type`. Retrying forever would
///   spam logs without changing the outcome.
/// - Best-effort async helpers (`handle_incoming_query`, `..._query_response`,
///   `..._schema_list_*`, `..._referral_*`): these already log their internal
///   failures and have no return signal; they're treated as `Ok(Handled)` once
///   the payload parses. Refactoring them to return `Result` is out of scope
///   for this fix and would explode the diff.
#[allow(clippy::too_many_lines)]
async fn dispatch_decrypted_message(
    node: &FoldNode,
    store: &dyn fold_db::storage::traits::KvStore,
    master_key: &[u8],
    publisher: &DiscoveryPublisher,
    msg: &crate::discovery::types::EncryptedMessage,
    raw: serde_json::Value,
) -> Result<DispatchOutcome, HandlerError> {
    let message_type = raw
        .get("message_type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match message_type.as_str() {
        "request" => {
            let payload: ConnectionPayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse connection request: {e}"),
                    });
                }
            };

            // Mutual contact detection via network intersection
            let mutual_contacts = if let Some(ref keys) = payload.network_keys {
                let op = OperationProcessor::new(std::sync::Arc::new(node.clone()));
                let our_book = op
                    .contact_book_path()
                    .ok()
                    .map(|p| ContactBook::load_from(&p).unwrap_or_default())
                    .unwrap_or_default();
                let our_keys: std::collections::HashSet<&str> = our_book
                    .active_contacts()
                    .iter()
                    .map(|c| c.public_key.as_str())
                    .collect();
                keys.iter()
                    .filter(|k| our_keys.contains(k.as_str()))
                    .filter_map(|k| {
                        our_book.get(k).map(|c| MutualContact {
                            display_name: c.display_name.clone(),
                            public_key: c.public_key.clone(),
                        })
                    })
                    .collect()
            } else {
                vec![]
            };

            let request_id = format!("msg-{}", msg.message_id);
            let local_req = LocalConnectionRequest {
                request_id,
                message_id: msg.message_id.clone(),
                target_pseudonym: msg.target_pseudonym.clone(),
                sender_pseudonym: payload.sender_pseudonym.clone(),
                sender_public_key: payload.sender_public_key.clone(),
                reply_public_key: payload.reply_public_key.clone(),
                message: payload.message.clone(),
                status: "pending".to_string(),
                created_at: msg.created_at.clone(),
                responded_at: None,
                vouches: Vec::new(),
                referral_query_id: None,
                referral_contacts_queried: 0,
                mutual_contacts,
                // Stash the requester's stable id so that when we later build
                // an accept message we can echo it back (G3 — stable match).
                sender_request_id: payload.request_id.clone(),
                // Stash the requester's stable identity pseudonym so we
                // can persist it on the contact row at accept-time and
                // use it as a referral-match key.
                sender_identity_pseudonym: payload.identity_pseudonym.clone(),
            };
            connection::save_received_request(store, &local_req)
                .await
                .map_err(|e| HandlerError::Internal(format!("save received request: {e}")))?;
            Ok(DispatchOutcome::Handled)
        }
        "accept" => {
            let payload: ConnectionPayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse connection accept: {e}"),
                    });
                }
            };
            let sent_request = connection::update_sent_request_status(
                store,
                &payload.sender_pseudonym,
                payload.request_id.as_deref(),
                "accepted",
            )
            .await
            .map_err(|e| HandlerError::Internal(format!("update sent request: {e}")))?;

            // Use the preferred_role from the original sent request, falling
            // back to "acquaintance" if unset or if the sent request wasn't found.
            let role = sent_request
                .as_ref()
                .and_then(|r| r.preferred_role.as_deref())
                .unwrap_or("acquaintance");

            // Auto-create trust relationship from accepted connection
            if payload.identity_card.is_some() {
                process_accepted_connection(node, &payload, role)
                    .await
                    .map_err(|e| {
                        HandlerError::Internal(format!("process accepted connection: {e}"))
                    })?;
            }
            Ok(DispatchOutcome::Handled)
        }
        "decline" => {
            let payload: ConnectionPayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse connection decline: {e}"),
                    });
                }
            };
            connection::update_sent_request_status(
                store,
                &payload.sender_pseudonym,
                payload.request_id.as_deref(),
                "declined",
            )
            .await
            .map_err(|e| HandlerError::Internal(format!("update sent request: {e}")))?;
            Ok(DispatchOutcome::Handled)
        }
        "query_request" => {
            let payload: QueryRequestPayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse query request: {e}"),
                    });
                }
            };
            handle_incoming_query(node, &payload, master_key, publisher).await
        }
        "query_response" => {
            let payload: QueryResponsePayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse query response: {e}"),
                    });
                }
            };
            handle_incoming_query_response(store, &payload).await
        }
        "schema_list_request" => {
            let payload: SchemaListRequestPayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse schema list request: {e}"),
                    });
                }
            };
            handle_incoming_schema_list_request(node, &payload, master_key, publisher).await
        }
        "schema_list_response" => {
            let payload: SchemaListResponsePayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse schema list response: {e}"),
                    });
                }
            };
            handle_incoming_schema_list_response(store, &payload).await
        }
        "data_share" => {
            let payload: DataSharePayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse data share: {e}"),
                    });
                }
            };

            // Part A: authorization gate — sender must be a known, non-revoked
            // contact. Unknown or revoked senders could otherwise inject
            // arbitrary mutations onto this node just by knowing our messaging
            // pseudonym + pubkey. See fix doc: trust-boundary hole.
            let op = OperationProcessor::new(std::sync::Arc::new(node.clone()));
            let book_path = op
                .contact_book_path()
                .map_err(|e| HandlerError::Internal(format!("contact book path: {e}")))?;
            let contact_book = ContactBook::load_from(&book_path).unwrap_or_default();
            match authorize_data_share_sender(&contact_book, &payload) {
                DataShareAuthz::Authorized => {}
                DataShareAuthz::UnknownSender => {
                    let pk_preview: String = payload.sender_public_key.chars().take(8).collect();
                    log::warn!(
                        "Rejecting data_share (msg {}): unknown sender pubkey {}",
                        msg.message_id,
                        pk_preview
                    );
                    return Ok(DispatchOutcome::Skipped {
                        reason: "data_share from unknown sender".to_string(),
                    });
                }
                DataShareAuthz::RevokedSender => {
                    let pk_preview: String = payload.sender_public_key.chars().take(8).collect();
                    log::warn!(
                        "Rejecting data_share (msg {}): revoked sender pubkey {}",
                        msg.message_id,
                        pk_preview
                    );
                    return Ok(DispatchOutcome::Skipped {
                        reason: "data_share from revoked sender".to_string(),
                    });
                }
            }

            // Part B: schema gate — schema service owns schema creation. Do
            // not allow a shared payload to install or auto-approve schemas.
            let db = node
                .get_fold_db()
                .map_err(|e| HandlerError::Internal(format!("Failed to get db: {e}")))?;
            let schema_states = db
                .schema_manager()
                .get_schema_states()
                .map_err(|e| HandlerError::Internal(format!("get schema states: {e}")))?;
            match validate_data_share_schemas(&schema_states, &payload) {
                DataShareSchemaCheck::AllApproved => {}
                DataShareSchemaCheck::UnknownOrUnapproved { schema_name } => {
                    log::warn!(
                        "Rejecting data_share (msg {}): schema '{}' not installed/approved on this node",
                        msg.message_id,
                        schema_name
                    );
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("data_share references unknown schema '{schema_name}'"),
                    });
                }
            }

            process_data_share(node, &payload).await?;
            log::info!(
                "Received {} records from {}",
                payload.records.len(),
                payload.sender_display_name
            );
            Ok(DispatchOutcome::Handled)
        }
        "share_invite" => {
            let payload: ShareInvitePayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse share_invite: {e}"),
                    });
                }
            };
            handle_incoming_share_invite(node, master_key, msg, payload).await
        }
        "referral_query" => {
            let payload: ReferralQueryPayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse referral query: {e}"),
                    });
                }
            };
            handle_incoming_referral_query(node, &payload, master_key, publisher).await
        }
        "referral_response" => {
            let payload: ReferralResponsePayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse referral response: {e}"),
                    });
                }
            };
            handle_incoming_referral_response(node, store, &payload).await
        }
        "identity_card_send" => {
            let payload: IdentityCardMessagePayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse identity_card_send: {e}"),
                    });
                }
            };
            handle_incoming_identity_card(store, &payload).await
        }
        other => Ok(DispatchOutcome::Skipped {
            reason: format!("unknown message_type '{other}'"),
        }),
    }
}

// ===== Share invite inbound =====

/// Handle an incoming `share_invite` bulletin-board message.
///
/// The outer envelope is already decrypted; `payload.share_e2e_secret_encrypted`
/// is still an X25519 sealed box to our messaging pseudonym secret. We decrypt
/// that inner layer and persist the plaintext invite to the pending-invites
/// queue so the user can explicitly accept it via `POST /api/sharing/accept`.
///
/// Authorization: invites from unknown pubkeys are still persisted — the user
/// is the gatekeeper via the accept step. A future iteration can add a
/// known-contacts-only filter via the contact book.
async fn handle_incoming_share_invite(
    node: &FoldNode,
    master_key: &[u8],
    msg: &crate::discovery::types::EncryptedMessage,
    payload: ShareInvitePayload,
) -> Result<DispatchOutcome, HandlerError> {
    let target: uuid::Uuid = match msg.target_pseudonym.parse() {
        Ok(u) => u,
        Err(_) => {
            return Ok(DispatchOutcome::Skipped {
                reason: "share_invite: target_pseudonym not a UUID".to_string(),
            });
        }
    };

    let (secret, _) = connection::derive_pseudonym_keypair(master_key, &target);

    // Decrypt the inner share_e2e_secret layer.
    let secret_value: serde_json::Value =
        match connection::decrypt_message_raw(&secret, &payload.share_e2e_secret_encrypted) {
            Ok(v) => v,
            Err(e) => {
                return Ok(DispatchOutcome::Skipped {
                    reason: format!("share_invite: inner decrypt failed: {e}"),
                });
            }
        };
    let secret_bytes: Vec<u8> = match serde_json::from_value(secret_value) {
        Ok(b) => b,
        Err(e) => {
            return Ok(DispatchOutcome::Skipped {
                reason: format!("share_invite: inner secret not a byte array: {e}"),
            });
        }
    };

    // Build the plaintext invite and stash it in the pending-invites queue.
    let invite = fold_db::sharing::types::ShareInvite {
        sender_pubkey: payload.sender_pubkey.clone(),
        sender_display_name: payload.sender_display_name.clone(),
        share_prefix: payload.share_prefix.clone(),
        share_e2e_secret: secret_bytes,
        scope_description: payload.scope_description.clone(),
    };

    let pool = match crate::handlers::sharing::get_sled_pool(node).await {
        Ok(p) => p,
        Err(e) => {
            return Err(HandlerError::Internal(format!(
                "share_invite: sled pool: {e}"
            )));
        }
    };

    if let Err(e) = fold_db::sharing::store::store_pending_invite(&pool, invite) {
        return Err(HandlerError::Internal(format!(
            "share_invite: persist failed: {e}"
        )));
    }

    log::info!(
        "Received share_invite from {} (prefix {})",
        payload.sender_pubkey,
        payload.share_prefix
    );
    Ok(DispatchOutcome::Handled)
}

// ===== Async query auto-processing helpers =====

/// Handle an incoming query request: execute the query and send results back.
/// Returns `Skipped` for permanent parse failures (bad reply pk, bad sender
/// pseudonym UUID — retrying won't recover). Returns `Err` for transient
/// network or encryption failures so the caller retries (FU-8).
async fn handle_incoming_query(
    node: &FoldNode,
    payload: &QueryRequestPayload,
    master_key: &[u8],
    publisher: &DiscoveryPublisher,
) -> Result<DispatchOutcome, HandlerError> {
    use crate::fold_node::OperationProcessor;
    use fold_db::schema::types::operations::Query;

    log::info!(
        "Processing incoming query request {} for schema '{}'",
        payload.request_id,
        payload.schema_name
    );

    let op = OperationProcessor::new(std::sync::Arc::new(node.clone()));
    let query = Query::new(payload.schema_name.clone(), payload.fields.clone());

    // Query execution errors are reported back to the caller in the response
    // payload — this is part of the wire protocol, not a silent failure.
    let (success, results, error) = match op
        .execute_query_json_with_access(query, &payload.sender_public_key)
        .await
    {
        Ok(results) => (true, Some(results), None),
        Err(e) => (false, None, Some(format!("Query failed: {}", e))),
    };

    let hash = crate::discovery::pseudonym::content_hash("connection-sender");
    let our_pseudonym = crate::discovery::pseudonym::derive_pseudonym(master_key, &hash);
    let our_reply_pk = connection::get_pseudonym_public_key_b64(master_key, &our_pseudonym);

    let response = QueryResponsePayload {
        message_type: "query_response".to_string(),
        request_id: payload.request_id.clone(),
        success,
        results,
        error,
        sender_pseudonym: our_pseudonym.to_string(),
        reply_public_key: our_reply_pk,
    };

    // Parse-permanent: reply public key malformed on the wire. Not recoverable.
    let reply_pk_bytes = match B64.decode(&payload.reply_public_key) {
        Ok(b) if b.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&b);
            arr
        }
        _ => {
            return Ok(DispatchOutcome::Skipped {
                reason: format!(
                    "invalid reply public key in query request {}",
                    payload.request_id
                ),
            });
        }
    };

    // Encryption failure is a transient crypto error — propagate for retry.
    let encrypted = connection::encrypt_message(&reply_pk_bytes, &response)
        .map_err(|e| HandlerError::Internal(format!("encrypt query response: {e}")))?;

    // Parse-permanent: sender pseudonym malformed UUID. Not recoverable.
    let sender_pseudonym: uuid::Uuid = match payload.sender_pseudonym.parse() {
        Ok(u) => u,
        Err(e) => {
            return Ok(DispatchOutcome::Skipped {
                reason: format!("invalid sender pseudonym UUID: {e}"),
            });
        }
    };

    let encrypted_b64 = B64.encode(&encrypted);
    publisher
        .connect(sender_pseudonym, encrypted_b64, Some(our_pseudonym))
        .await
        .map_err(|e| HandlerError::Internal(format!("send query response: {e}")))?;
    Ok(DispatchOutcome::Handled)
}

/// Handle an incoming query response: update local async query with results.
/// Returns `Err` on transient store failure so the dispatcher retries
/// (FU-8: was previously swallowing the error silently).
/// Handle an incoming query response: update the local async query row.
/// Returns `Ok(DispatchOutcome::Skipped)` when the referenced `request_id`
/// no longer exists locally (permanent — retrying forever won't bring it
/// back; it was pruned or never sent from this node). Propagates transient
/// store errors as `Err` so the dispatcher retries, instead of silently
/// dropping the response (FU-8).
async fn handle_incoming_query_response(
    store: &dyn fold_db::storage::traits::KvStore,
    payload: &QueryResponsePayload,
) -> Result<DispatchOutcome, HandlerError> {
    log::info!("Received query response for request {}", payload.request_id);

    // Check existence first so "unknown request_id" is classified as a
    // permanent skip rather than a retriable error. Load failure itself is
    // transient and propagates.
    let existing = async_query::get_async_query(store, &payload.request_id)
        .await
        .map_err(|e| HandlerError::Internal(format!("load async query: {e}")))?;
    if existing.is_none() {
        log::warn!(
            "Query response for unknown request {} (pruned or never sent from this node)",
            payload.request_id
        );
        return Ok(DispatchOutcome::Skipped {
            reason: format!("unknown async query request_id {}", payload.request_id),
        });
    }

    let results = if payload.success {
        match payload.results.as_ref() {
            Some(r) => Some(
                serde_json::to_value(r)
                    .map_err(|e| HandlerError::Internal(format!("serialize query results: {e}")))?,
            ),
            None => None,
        }
    } else {
        None
    };

    async_query::update_async_query_result(
        store,
        &payload.request_id,
        results,
        payload.error.clone(),
    )
    .await
    .map_err(|e| HandlerError::Internal(format!("update async query result: {e}")))?;
    Ok(DispatchOutcome::Handled)
}

/// Handle an incoming `identity_card_send` payload: write a pending
/// inbox row to Sled and leave signature verification for the user's
/// Accept click. Dropped (`Skipped`) when `sender_public_key` doesn't
/// match the pub_key the card claims — that's the "Bob forwards
/// Alice's card under his pseudonym" replay, and we don't want it in
/// the user's inbox at all.
///
/// We deliberately do NOT fail the poll on a parse issue in the
/// inner card: the row still lands so the user sees "Alice sent
/// something I couldn't read" rather than silent data loss.
async fn handle_incoming_identity_card(
    store: &dyn fold_db::storage::traits::KvStore,
    payload: &IdentityCardMessagePayload,
) -> Result<DispatchOutcome, HandlerError> {
    // Anti-replay gate: the sender's bulletin-board identity (pubkey
    // they signed the message envelope with, captured as
    // sender_public_key) must match the pubkey embedded in the card.
    // A node can't sign another node's card — that's the whole point
    // of the Ed25519 signature — but a node CAN wrap another's card
    // in their own envelope and try to pass it off. Dropping those
    // at dispatch time keeps the inbox honest.
    let card_pub_key = payload
        .card
        .get("pub_key")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if card_pub_key != payload.sender_public_key {
        log::warn!(
            "Rejecting identity_card_send (msg {}): card.pub_key='{}' does not match sender_public_key='{}'",
            payload.message_id,
            card_pub_key,
            payload.sender_public_key
        );
        return Ok(DispatchOutcome::Skipped {
            reason: "identity_card_send: card.pub_key != sender_public_key".to_string(),
        });
    }

    let row = LocalReceivedCard {
        message_id: payload.message_id.clone(),
        card: payload.card.clone(),
        sender_public_key: payload.sender_public_key.clone(),
        sender_pseudonym: payload.sender_pseudonym.clone(),
        status: "pending".to_string(),
        received_at: chrono::Utc::now().to_rfc3339(),
        resolved_at: None,
        accepted_identity_id: None,
        error: None,
    };
    received_card::save_received_card(store, &row)
        .await
        .map_err(|e| HandlerError::Internal(format!("save received card: {e}")))?;
    log::info!(
        "fingerprints.inbound: stored identity_card_send (msg_id={}) from pubkey='{}' as pending",
        payload.message_id,
        payload.sender_public_key,
    );
    Ok(DispatchOutcome::Handled)
}

/// Handle an incoming schema list request: list schemas and send back.
/// Returns `Skipped` for wire-malformed parse-permanent errors; returns `Err`
/// for transient db/network failures so the caller retries (FU-8).
async fn handle_incoming_schema_list_request(
    node: &FoldNode,
    payload: &SchemaListRequestPayload,
    master_key: &[u8],
    publisher: &DiscoveryPublisher,
) -> Result<DispatchOutcome, HandlerError> {
    use crate::fold_node::OperationProcessor;

    log::info!(
        "Processing incoming schema list request {}",
        payload.request_id
    );

    let op = OperationProcessor::new(std::sync::Arc::new(node.clone()));
    let db = op
        .get_db_public()
        .map_err(|e| HandlerError::Internal(format!("get db for schema list: {e}")))?;

    let all_schemas = db
        .schema_manager()
        .get_schemas()
        .map_err(|e| HandlerError::Internal(format!("get schemas: {e}")))?;
    let schemas: Vec<SchemaInfo> = all_schemas
        .values()
        .map(|s| SchemaInfo {
            name: s.name.clone(),
            descriptive_name: s.descriptive_name.clone(),
        })
        .collect();

    let hash = crate::discovery::pseudonym::content_hash("connection-sender");
    let our_pseudonym = crate::discovery::pseudonym::derive_pseudonym(master_key, &hash);
    let our_reply_pk = connection::get_pseudonym_public_key_b64(master_key, &our_pseudonym);

    let response = SchemaListResponsePayload {
        message_type: "schema_list_response".to_string(),
        request_id: payload.request_id.clone(),
        schemas,
        sender_pseudonym: our_pseudonym.to_string(),
        reply_public_key: our_reply_pk,
    };

    let reply_pk_bytes = match B64.decode(&payload.reply_public_key) {
        Ok(b) if b.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&b);
            arr
        }
        _ => {
            return Ok(DispatchOutcome::Skipped {
                reason: "invalid reply public key in schema list request".to_string(),
            });
        }
    };

    let encrypted = connection::encrypt_message(&reply_pk_bytes, &response)
        .map_err(|e| HandlerError::Internal(format!("encrypt schema list response: {e}")))?;

    let sender_pseudonym: uuid::Uuid = match payload.sender_pseudonym.parse() {
        Ok(u) => u,
        Err(e) => {
            return Ok(DispatchOutcome::Skipped {
                reason: format!("invalid sender pseudonym UUID in schema list request: {e}"),
            });
        }
    };

    let encrypted_b64 = B64.encode(&encrypted);
    publisher
        .connect(sender_pseudonym, encrypted_b64, Some(our_pseudonym))
        .await
        .map_err(|e| HandlerError::Internal(format!("send schema list response: {e}")))?;
    Ok(DispatchOutcome::Handled)
}

/// Handle an incoming schema list response: update local async query.
/// Returns `Ok(DispatchOutcome::Skipped)` when the referenced `request_id`
/// no longer exists locally (permanent — pruned or never sent from this
/// node). Propagates transient store errors as `Err` so the dispatcher
/// retries, instead of silently swallowing them (FU-8).
async fn handle_incoming_schema_list_response(
    store: &dyn fold_db::storage::traits::KvStore,
    payload: &SchemaListResponsePayload,
) -> Result<DispatchOutcome, HandlerError> {
    log::info!(
        "Received schema list response for request {}",
        payload.request_id
    );

    let existing = async_query::get_async_query(store, &payload.request_id)
        .await
        .map_err(|e| HandlerError::Internal(format!("load async query: {e}")))?;
    if existing.is_none() {
        log::warn!(
            "Schema list response for unknown request {} (pruned or never sent from this node)",
            payload.request_id
        );
        return Ok(DispatchOutcome::Skipped {
            reason: format!(
                "unknown async schema list request_id {}",
                payload.request_id
            ),
        });
    }

    let results = serde_json::to_value(&payload.schemas)
        .map_err(|e| HandlerError::Internal(format!("serialize schema list: {e}")))?;
    async_query::update_async_query_result(store, &payload.request_id, Some(results), None)
        .await
        .map_err(|e| HandlerError::Internal(format!("update schema list result: {e}")))?;
    Ok(DispatchOutcome::Handled)
}

/// Process an accepted connection: create trust relationship and contact on our side.
/// Called when the polling loop receives an "accept" message with an identity card.
async fn process_accepted_connection(
    node: &FoldNode,
    acceptance: &ConnectionPayload,
    default_role: &str,
) -> Result<(), HandlerError> {
    let identity = acceptance
        .identity_card
        .as_ref()
        .ok_or_else(|| HandlerError::Internal("No identity card in acceptance".to_string()))?;

    let op = OperationProcessor::new(std::sync::Arc::new(node.clone()));
    let roles_path = op
        .sharing_roles_path()
        .map_err(|e| HandlerError::Internal(format!("Failed to resolve roles path: {e}")))?;
    let config = SharingRoleConfig::load_from(&roles_path)
        .map_err(|e| HandlerError::Internal(format!("Failed to load roles: {e}")))?;
    let role = config
        .get_role(default_role)
        .ok_or_else(|| HandlerError::Internal(format!("Unknown role: {default_role}")))?;

    op.grant_trust_for_domain(&identity.node_public_key, &role.domain, role.tier)
        .await
        .map_err(|e: fold_db::schema::SchemaError| {
            HandlerError::Internal(format!("Failed to grant trust: {e}"))
        })?;

    let contact = Contact::from_discovery(
        identity.node_public_key.clone(),
        identity.display_name.clone(),
        identity.contact_hint.clone(),
        TrustDirection::Outgoing,
        Some(acceptance.sender_pseudonym.clone()),
        Some(acceptance.reply_public_key.clone()),
        acceptance.identity_pseudonym.clone(),
        role.domain.clone(),
        default_role.to_string(),
    );
    let book_path = op
        .contact_book_path()
        .map_err(|e| HandlerError::Internal(format!("Failed to resolve contacts path: {e}")))?;
    let mut book = ContactBook::load_from(&book_path)
        .map_err(|e| HandlerError::Internal(format!("Failed to load contacts: {e}")))?;
    book.upsert_contact(contact);
    book.save_to(&book_path)
        .map_err(|e| HandlerError::Internal(format!("Failed to save contacts: {e}")))?;

    Ok(())
}

///    the same identity pseudonym for them, so this is the reliable key.
/// 2. **Legacy fallback** — rotating pseudonym / public-key fields.
///    Used for contacts that were created before identity pseudonyms
///    existed, or when the referral query came from a legacy sender.
fn referral_contact_matches(c: &Contact, payload: &ReferralQueryPayload) -> bool {
    let identity_match = match (
        c.identity_pseudonym.as_deref(),
        payload.subject_identity_pseudonym.as_deref(),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    identity_match
        || c.pseudonym.as_deref() == Some(&payload.subject_pseudonym)
        || c.messaging_pseudonym.as_deref() == Some(&payload.subject_pseudonym)
        || c.messaging_public_key.as_deref() == Some(&payload.subject_public_key)
}

/// Pure match helper: does contact `c` correspond to the voucher who sent
/// referral `response`?
///
/// Same primary/fallback strategy as [`referral_contact_matches`].
fn referral_voucher_matches(c: &Contact, response: &ReferralResponsePayload) -> bool {
    let identity_match = match (
        c.identity_pseudonym.as_deref(),
        response.voucher_identity_pseudonym.as_deref(),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    identity_match
        || c.messaging_pseudonym.as_deref() == Some(&response.sender_pseudonym)
        || c.pseudonym.as_deref() == Some(&response.sender_pseudonym)
}

/// Handle an incoming referral query: check if we know the subject and respond if so.
/// Returns `Skipped` for parse-permanent errors; `Err` for transient network
/// or crypto failures so the caller retries (FU-8).
async fn handle_incoming_referral_query(
    node: &FoldNode,
    payload: &ReferralQueryPayload,
    master_key: &[u8],
    publisher: &DiscoveryPublisher,
) -> Result<DispatchOutcome, HandlerError> {
    let op = OperationProcessor::new(std::sync::Arc::new(node.clone()));
    let book_path = op
        .contact_book_path()
        .map_err(|e| HandlerError::Internal(format!("contact book path: {e}")))?;
    let contact_book = ContactBook::load_from(&book_path).unwrap_or_default();

    // Check if we know the subject. See `referral_contact_matches` for the
    // primary/legacy-fallback match strategy.
    let active = contact_book.active_contacts();
    let matched_contact = active.iter().find(|c| referral_contact_matches(c, payload));

    let contact = match matched_contact {
        Some(c) => (*c).clone(),
        None => {
            // Silence = no. This is the protocol: non-matching referral queries
            // produce no response. Mark as handled (nothing to retry).
            return Ok(DispatchOutcome::Handled);
        }
    };

    // Derive our connection-sender pseudonym + X25519 key
    let sender_hash = crate::discovery::pseudonym::content_hash("connection-sender");
    let our_pseudonym = crate::discovery::pseudonym::derive_pseudonym(master_key, &sender_hash);
    let our_pk_b64 = connection::get_pseudonym_public_key_b64(master_key, &our_pseudonym);

    let response = ReferralResponsePayload {
        message_type: "referral_response".to_string(),
        query_id: payload.query_id.clone(),
        known_as: contact.display_name.clone(),
        sender_pseudonym: our_pseudonym.to_string(),
        reply_public_key: our_pk_b64,
        // Our own stable identity pseudonym so the query originator can
        // resolve us to a contact row and render our display name
        // instead of "Unknown contact".
        voucher_identity_pseudonym: Some(
            crate::discovery::pseudonym::derive_identity_pseudonym(master_key).to_string(),
        ),
    };

    let reply_pk_bytes = match B64.decode(&payload.reply_public_key) {
        Ok(b) if b.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&b);
            arr
        }
        Ok(_) => {
            return Ok(DispatchOutcome::Skipped {
                reason: "referral query reply key wrong length".to_string(),
            });
        }
        Err(e) => {
            return Ok(DispatchOutcome::Skipped {
                reason: format!("invalid referral query reply key: {e}"),
            });
        }
    };

    let encrypted = connection::encrypt_message(&reply_pk_bytes, &response)
        .map_err(|e| HandlerError::Internal(format!("encrypt referral response: {e}")))?;

    let encrypted_b64 = B64.encode(&encrypted);

    let sender_uuid: uuid::Uuid = match payload.sender_pseudonym.parse() {
        Ok(u) => u,
        Err(e) => {
            return Ok(DispatchOutcome::Skipped {
                reason: format!("invalid sender pseudonym UUID in referral query: {e}"),
            });
        }
    };

    publisher
        .connect(sender_uuid, encrypted_b64, Some(our_pseudonym))
        .await
        .map_err(|e| HandlerError::Internal(format!("send referral response: {e}")))?;
    log::info!(
        "Sent referral response for query {} (known as {})",
        payload.query_id,
        contact.display_name
    );
    Ok(DispatchOutcome::Handled)
}

/// Handle an incoming referral response: append the vouch to the connection request.
/// Returns `Ok(DispatchOutcome::Skipped)` when the referenced connection request
/// no longer exists (permanent — retrying won't help). Propagates transient
/// store errors as `Err` so the dispatcher retries, instead of silently
/// dropping the vouch (FU-8).
async fn handle_incoming_referral_response(
    node: &FoldNode,
    store: &dyn fold_db::storage::traits::KvStore,
    payload: &ReferralResponsePayload,
) -> Result<DispatchOutcome, HandlerError> {
    // Scan for the connection request matching this query_id. A scan failure
    // here is transient — propagate so the caller retries.
    let entries = store
        .scan_prefix(b"discovery:conn_req:")
        .await
        .map_err(|e| HandlerError::Internal(format!("scan connection requests: {e}")))?;

    let mut found_key: Option<Vec<u8>> = None;
    let mut found_req: Option<LocalConnectionRequest> = None;
    for (key, value) in &entries {
        // Deserialization of an existing row: if one row is corrupt, log and
        // skip just that row — matching connection.rs's existing pattern. Do
        // NOT drop the whole message because of one bad neighbor.
        match serde_json::from_slice::<LocalConnectionRequest>(value) {
            Ok(local_req) => {
                if local_req.referral_query_id.as_deref() == Some(&payload.query_id) {
                    found_key = Some(key.clone());
                    found_req = Some(local_req);
                    break;
                }
            }
            Err(e) => {
                log::warn!("Corrupt connection request row during referral scan: {e}");
            }
        }
    }

    let (sled_key, mut local_req) = match (found_key, found_req) {
        (Some(k), Some(r)) => (k, r),
        _ => {
            log::warn!("Referral response for unknown query {}", payload.query_id);
            return Ok(DispatchOutcome::Skipped {
                reason: format!("referral response for unknown query {}", payload.query_id),
            });
        }
    };

    // Look up voucher identity from contact book. Contact-book load failure
    // is treated as "no voucher name"; this is deliberate because the vouch
    // itself is the important part and we already have the sender pseudonym
    // on the payload. We do NOT hide the error from the caller — we just
    // render an explicit "Unknown contact" label.
    let voucher_display_name = {
        let op = OperationProcessor::new(std::sync::Arc::new(node.clone()));
        match op.contact_book_path() {
            Ok(book_path) => {
                let contact_book = ContactBook::load_from(&book_path).unwrap_or_default();
                contact_book
                    .active_contacts()
                    .iter()
                    .find(|c| referral_voucher_matches(c, payload))
                    .map(|c| c.display_name.clone())
                    .unwrap_or_else(|| "Unknown contact".to_string())
            }
            Err(e) => {
                log::warn!("Failed to resolve contact book path for voucher lookup: {e}");
                "Unknown contact".to_string()
            }
        }
    };

    local_req.vouches.push(Vouch {
        voucher_display_name,
        known_as: payload.known_as.clone(),
        received_at: chrono::Utc::now().to_rfc3339(),
    });

    let updated = serde_json::to_vec(&local_req).map_err(|e| {
        HandlerError::Internal(format!("serialize vouched connection request: {e}"))
    })?;

    store
        .put(&sled_key, updated)
        .await
        .map_err(|e| HandlerError::Internal(format!("save vouched connection request: {e}")))?;
    log::info!(
        "Added vouch for referral query {} (known as '{}')",
        payload.query_id,
        payload.known_as
    );
    Ok(DispatchOutcome::Handled)
}

/// Outcome of the data-share sender authorization check.
///
/// Pure-function result so the gate can be unit tested without standing up a
/// full `FoldNode`. See [`authorize_data_share_sender`].
#[derive(Debug, PartialEq, Eq)]
enum DataShareAuthz {
    /// Sender matches an active (non-revoked) contact.
    Authorized,
    /// Sender pubkey does not match any contact in the book.
    UnknownSender,
    /// Sender matches a contact whose trust has been revoked.
    RevokedSender,
}

/// Pure authorization helper: does `payload.sender_public_key` correspond to
/// a known, non-revoked contact in `contact_book`?
///
/// The primary match key is the Ed25519 pubkey (base64) because that is the
/// field already carried on every [`DataSharePayload`]. If the contact also
/// carries a stable `identity_pseudonym` (PR #418) and the payload ever grows
/// one, that would become the secondary key — for now only the pubkey match
/// is available and it is both necessary and sufficient.
fn authorize_data_share_sender(
    contact_book: &ContactBook,
    payload: &DataSharePayload,
) -> DataShareAuthz {
    match contact_book.get(&payload.sender_public_key) {
        None => DataShareAuthz::UnknownSender,
        Some(c) if c.revoked => DataShareAuthz::RevokedSender,
        Some(_) => DataShareAuthz::Authorized,
    }
}

/// Outcome of the data-share schema validation check.
#[derive(Debug, PartialEq, Eq)]
enum DataShareSchemaCheck {
    /// Every record references a schema that is already installed and
    /// approved on this node.
    AllApproved,
    /// At least one record references a schema the node has not installed
    /// and approved. The first offending schema name is returned so the
    /// caller can log it.
    UnknownOrUnapproved { schema_name: String },
}

/// Pure schema-validation helper: every record in `payload` must reference a
/// schema that is already present on this node AND in the `Approved` state.
///
/// Schema creation is owned by the schema service. Shared payloads must NOT
/// be able to install or auto-approve arbitrary schema definitions — doing
/// so would let any contact silently add a schema to the recipient's node.
fn validate_data_share_schemas(
    schema_states: &HashMap<String, fold_db::schema::SchemaState>,
    payload: &DataSharePayload,
) -> DataShareSchemaCheck {
    for record in &payload.records {
        match schema_states.get(&record.schema_name) {
            Some(fold_db::schema::SchemaState::Approved) => {}
            _ => {
                return DataShareSchemaCheck::UnknownOrUnapproved {
                    schema_name: record.schema_name.clone(),
                };
            }
        }
    }
    DataShareSchemaCheck::AllApproved
}

/// Process a received data share: write mutations and save any included
/// file data.
///
/// Caller MUST have already authorized the sender (via
/// [`authorize_data_share_sender`]) and validated that every referenced
/// schema is installed and approved (via [`validate_data_share_schemas`]).
/// This function assumes both gates have passed and does not re-check.
async fn process_data_share(
    node: &FoldNode,
    payload: &DataSharePayload,
) -> Result<(), HandlerError> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to get db: {e}")))?;

    for record in &payload.records {
        // Write the mutation with the sender's pub_key. The dispatch arm has
        // already verified the schema is installed and approved, so any
        // failure here is a transient Sled hiccup — propagate so the caller
        // retries on the next poll instead of silently dropping the record.
        let key = fold_db::schema::types::key_value::KeyValue::new(
            record.key.hash.clone(),
            record.key.range.clone(),
        );

        let mutation = fold_db::schema::types::Mutation::new(
            record.schema_name.clone(),
            record.fields.clone(),
            key.clone(),
            payload.sender_public_key.clone(),
            fold_db::schema::types::operations::MutationType::Create,
        );

        db.mutation_manager()
            .write_mutations_batch_async(vec![mutation])
            .await
            .map_err(|e| {
                HandlerError::Internal(format!(
                    "write shared record for schema '{}': {e}",
                    record.schema_name
                ))
            })?;

        // If file data is included, save it to upload storage. A bad base64
        // blob is a permanent payload error — bail out with Internal so the
        // full partial write stays out of dedup and the peer can resend a
        // fixed payload. A Sled/fs write failure is transient — also bail.
        if let Some(ref file_b64) = record.file_data_base64 {
            let file_bytes = B64.decode(file_b64).map_err(|e| {
                HandlerError::Internal(format!(
                    "decode shared file data for schema '{}': {e}",
                    record.schema_name
                ))
            })?;
            let file_name = record
                .file_name
                .as_deref()
                .or_else(|| record.fields.get("file_hash").and_then(|v| v.as_str()))
                .unwrap_or("shared_file");

            let upload_path = std::env::var("FOLDDB_UPLOAD_PATH")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("data/uploads"));
            let upload_storage = fold_db::storage::UploadStorage::local(upload_path);

            upload_storage
                .save_file(file_name, &file_bytes, None)
                .await
                .map_err(|e| {
                    HandlerError::Internal(format!("save shared file '{file_name}': {e}"))
                })?;

            // Run face detection on shared photos. The mutation + file write
            // already succeeded, but face indexing is part of how this record
            // becomes searchable on the receiver. Treat failures as TRANSIENT
            // and propagate `Internal` so the dispatch arm retries on the
            // next poll cycle (matches the DispatchOutcome retry semantics
            // from PR #398). Silently warning would leave the record on the
            // node but invisible to face search forever.
            #[cfg(feature = "face-detection")]
            {
                let db_ops = db.get_db_ops();
                if let Some(native_idx) = db_ops.native_index_manager() {
                    if native_idx.has_face_processor() {
                        let count = native_idx
                            .index_faces(&record.schema_name, &key, &file_bytes)
                            .await
                            .map_err(|e| {
                                HandlerError::Internal(format!(
                                    "face indexing failed for shared photo '{file_name}': {e}"
                                ))
                            })?;
                        if count > 0 {
                            log::info!(
                                "Detected {} face(s) in shared photo '{}'",
                                count,
                                file_name
                            );
                        }
                    }
                }
            }
        }
    }

    // Store a notification for the UI. Failure here is transient (Sled) and
    // leaves the records written but the UI unnotified — propagate so the
    // poll retry can try again.
    let notification = serde_json::json!({
        "type": "data_share_received",
        "sender_display_name": payload.sender_display_name,
        "sender_public_key": payload.sender_public_key,
        "records_received": payload.records.len(),
        "schema_names": payload.records.iter().map(|r| r.schema_name.clone()).collect::<Vec<_>>(),
        "received_at": chrono::Utc::now().to_rfc3339(),
    });

    let notif_key = format!(
        "notification:{}:{}",
        chrono::Utc::now().timestamp_millis(),
        uuid::Uuid::new_v4()
    );
    let store = get_metadata_store(&db);
    store
        .put(
            notif_key.as_bytes(),
            serde_json::to_vec(&notification).map_err(|e| {
                HandlerError::Internal(format!("serialize data share notification: {e}"))
            })?,
        )
        .await
        .map_err(|e| HandlerError::Internal(format!("store data share notification: {e}")))?;

    log::info!(
        "Received {} records from {} (schemas: {})",
        payload.records.len(),
        payload.sender_display_name,
        payload
            .records
            .iter()
            .map(|r| r.schema_name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    Ok(())
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use crate::discovery::types::EncryptedMessage;
    use fold_db::storage::error::StorageResult;
    use fold_db::storage::inmemory_backend::InMemoryKvStore;
    use fold_db::storage::traits::{ExecutionModel, FlushBehavior, KvStore};
    use std::sync::Arc;
    use tempfile::tempdir;

    /// Build a minimal FoldNode suitable for tests that only touch the
    /// metadata store via the dispatch helper.
    async fn make_test_node() -> (FoldNode, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
        let config = crate::fold_node::NodeConfig::new(dir.path().to_path_buf())
            .with_schema_service_url("test://mock")
            .with_identity(&keypair.public_key_base64(), &keypair.secret_key_base64());
        let node = FoldNode::new(config).await.unwrap();
        (node, dir)
    }

    fn make_publisher() -> DiscoveryPublisher {
        DiscoveryPublisher::new(
            vec![0u8; 32],
            "test://mock".to_string(),
            "test-token".to_string(),
        )
    }

    fn make_msg(message_id: &str) -> EncryptedMessage {
        EncryptedMessage {
            message_id: message_id.to_string(),
            encrypted_blob: String::new(),
            target_pseudonym: uuid::Uuid::new_v4().to_string(),
            sender_pseudonym: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// A KvStore wrapper that fails `put` for keys starting with a configured
    /// prefix. Used to simulate transient storage failures for specific
    /// sub-operations without breaking the dedup marker write.
    struct FailingPutStore {
        inner: Arc<InMemoryKvStore>,
        fail_prefix: Vec<u8>,
    }

    #[async_trait::async_trait]
    impl KvStore for FailingPutStore {
        async fn get(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
            self.inner.get(key).await
        }
        async fn put(&self, key: &[u8], value: Vec<u8>) -> StorageResult<()> {
            if key.starts_with(&self.fail_prefix) {
                return Err(fold_db::storage::error::StorageError::BackendError(
                    "injected transient failure".to_string(),
                ));
            }
            self.inner.put(key, value).await
        }
        async fn delete(&self, key: &[u8]) -> StorageResult<bool> {
            self.inner.delete(key).await
        }
        async fn exists(&self, key: &[u8]) -> StorageResult<bool> {
            self.inner.exists(key).await
        }
        async fn scan_prefix(&self, prefix: &[u8]) -> StorageResult<Vec<(Vec<u8>, Vec<u8>)>> {
            self.inner.scan_prefix(prefix).await
        }
        async fn batch_put(&self, items: Vec<(Vec<u8>, Vec<u8>)>) -> StorageResult<()> {
            self.inner.batch_put(items).await
        }
        async fn batch_delete(&self, keys: Vec<Vec<u8>>) -> StorageResult<()> {
            self.inner.batch_delete(keys).await
        }
        async fn flush(&self) -> StorageResult<()> {
            self.inner.flush().await
        }
        fn backend_name(&self) -> &'static str {
            "failing-put-test"
        }
        fn execution_model(&self) -> ExecutionModel {
            self.inner.execution_model()
        }
        fn flush_behavior(&self) -> FlushBehavior {
            self.inner.flush_behavior()
        }
    }

    /// Parse-permanent: unknown message_type → Skipped (dedup should be marked
    /// by the caller so we don't re-log garbage forever).
    #[tokio::test]
    async fn unknown_message_type_is_skipped_not_errored() {
        let (node, _dir) = make_test_node().await;
        let publisher = make_publisher();
        let store: Arc<dyn KvStore> = Arc::new(InMemoryKvStore::new());
        let raw = serde_json::json!({"message_type": "totally_made_up"});
        let msg = make_msg("m-unknown");

        let outcome = dispatch_decrypted_message(&node, &*store, &[0u8; 32], &publisher, &msg, raw)
            .await
            .expect("unknown type must not return Err (it's a permanent skip)");

        match outcome {
            DispatchOutcome::Skipped { reason } => {
                assert!(
                    reason.contains("totally_made_up"),
                    "reason should name the unknown type, got: {reason}"
                );
            }
            DispatchOutcome::Handled => panic!("expected Skipped, got Handled"),
        }
    }

    /// Parse-permanent: malformed payload for a known message_type → Skipped.
    #[tokio::test]
    async fn malformed_request_payload_is_skipped() {
        let (node, _dir) = make_test_node().await;
        let publisher = make_publisher();
        let store: Arc<dyn KvStore> = Arc::new(InMemoryKvStore::new());
        // "request" expects ConnectionPayload fields; give it only message_type.
        let raw = serde_json::json!({"message_type": "request"});
        let msg = make_msg("m-bad-request");

        let outcome = dispatch_decrypted_message(&node, &*store, &[0u8; 32], &publisher, &msg, raw)
            .await
            .expect("parse failure must not return Err");

        assert!(matches!(outcome, DispatchOutcome::Skipped { .. }));
    }

    /// Happy path: a well-formed "request" payload is persisted and the
    /// dispatch returns Handled. A second identical dispatch (simulating the
    /// outer loop's dedup check) would be skipped by the caller via the dedup
    /// key — we verify the save landed in the store.
    #[tokio::test]
    async fn valid_request_is_handled_and_persisted() {
        let (node, _dir) = make_test_node().await;
        let publisher = make_publisher();
        let store: Arc<dyn KvStore> = Arc::new(InMemoryKvStore::new());

        let raw = serde_json::json!({
            "message_type": "request",
            "message": "hi",
            "sender_public_key": "pk1",
            "sender_pseudonym": "ps1",
            "reply_public_key": "rpk1",
        });
        let msg = make_msg("m-good");

        let outcome = dispatch_decrypted_message(&node, &*store, &[0u8; 32], &publisher, &msg, raw)
            .await
            .expect("valid request should succeed");

        assert!(matches!(outcome, DispatchOutcome::Handled));

        let received = connection::list_received_requests(&*store)
            .await
            .expect("list");
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].message_id, "m-good");
        assert_eq!(received[0].sender_pseudonym, "ps1");
    }

    /// Transient: `save_received_request` fails → dispatch returns Err. The
    /// outer loop uses this as the signal to NOT mark the dedup key, so the
    /// next poll can retry. We verify the error is returned AND that no
    /// connection-request was persisted (the failing put wrapper refused it).
    #[tokio::test]
    async fn transient_save_failure_returns_err_and_does_not_persist() {
        let (node, _dir) = make_test_node().await;
        let publisher = make_publisher();
        let inner = Arc::new(InMemoryKvStore::new());
        let store: Arc<dyn KvStore> = Arc::new(FailingPutStore {
            inner: inner.clone(),
            fail_prefix: b"discovery:conn_req:".to_vec(),
        });

        let raw = serde_json::json!({
            "message_type": "request",
            "message": "hi",
            "sender_public_key": "pk1",
            "sender_pseudonym": "ps1",
            "reply_public_key": "rpk1",
        });
        let msg = make_msg("m-transient");

        let outcome =
            dispatch_decrypted_message(&node, &*store, &[0u8; 32], &publisher, &msg, raw).await;

        match outcome {
            Err(HandlerError::Internal(msg)) => {
                assert!(
                    msg.contains("save received request"),
                    "error should identify the failing step, got: {msg}"
                );
            }
            other => panic!("expected Err(Internal), got {other:?}"),
        }

        // Nothing was persisted, so a subsequent retry would try again.
        let entries = inner
            .scan_prefix(b"discovery:conn_req:")
            .await
            .expect("scan");
        assert!(entries.is_empty(), "nothing should have been persisted");
    }

    /// Transient path + simulated outer-loop dedup behavior: verify the
    /// contract that Err leaves dedup absent and the next attempt re-runs
    /// dispatch. Models the key invariant from the bugfix.
    #[tokio::test]
    async fn err_leaves_dedup_absent_handled_sets_it() {
        let (node, _dir) = make_test_node().await;
        let publisher = make_publisher();
        let inner = Arc::new(InMemoryKvStore::new());

        // First attempt: store fails on save → Err, no dedup set.
        let failing: Arc<dyn KvStore> = Arc::new(FailingPutStore {
            inner: inner.clone(),
            fail_prefix: b"discovery:conn_req:".to_vec(),
        });
        let raw1 = serde_json::json!({
            "message_type": "request",
            "message": "hi",
            "sender_public_key": "pk1",
            "sender_pseudonym": "ps1",
            "reply_public_key": "rpk1",
        });
        let msg = make_msg("m-retry");
        let dedup_key = format!("msg_processed:{}", msg.message_id);

        let outcome1 =
            dispatch_decrypted_message(&node, &*failing, &[0u8; 32], &publisher, &msg, raw1).await;
        assert!(outcome1.is_err());
        // Outer loop would NOT have written the dedup key on Err.
        let dedup_present = inner.get(dedup_key.as_bytes()).await.unwrap();
        assert!(
            dedup_present.is_none(),
            "dedup key must be absent after transient failure"
        );

        // Second attempt: store is now healthy → Handled, caller sets dedup.
        let healthy: Arc<dyn KvStore> = inner.clone();
        let raw2 = serde_json::json!({
            "message_type": "request",
            "message": "hi",
            "sender_public_key": "pk1",
            "sender_pseudonym": "ps1",
            "reply_public_key": "rpk1",
        });
        let outcome2 =
            dispatch_decrypted_message(&node, &*healthy, &[0u8; 32], &publisher, &msg, raw2)
                .await
                .expect("second attempt should succeed");
        assert!(matches!(outcome2, DispatchOutcome::Handled));

        // Simulate the outer loop writing the dedup marker after Handled.
        healthy
            .put(dedup_key.as_bytes(), b"1".to_vec())
            .await
            .unwrap();
        let dedup_present = inner.get(dedup_key.as_bytes()).await.unwrap();
        assert_eq!(dedup_present.as_deref(), Some(&b"1"[..]));
    }

    /// Build a pending LocalAsyncQuery row so the response handlers have
    /// something to update. Kept minimal — only fields exercised by the
    /// response path.
    async fn seed_async_query(store: &dyn KvStore, request_id: &str, query_type: &str) {
        let q = crate::discovery::async_query::LocalAsyncQuery {
            request_id: request_id.to_string(),
            contact_public_key: "pk1".to_string(),
            contact_display_name: "Alice".to_string(),
            schema_name: Some("test_schema".to_string()),
            fields: vec![],
            query_type: query_type.to_string(),
            status: "pending".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
            results: None,
            error: None,
        };
        crate::discovery::async_query::save_async_query(store, &q)
            .await
            .expect("seed async query");
    }

    /// Happy path: query response for a known request_id updates the row
    /// and returns Handled.
    #[tokio::test]
    async fn query_response_happy_path_is_handled() {
        let store: Arc<dyn KvStore> = Arc::new(InMemoryKvStore::new());
        seed_async_query(&*store, "req-123", "query").await;

        let payload = QueryResponsePayload {
            message_type: "query_response".to_string(),
            request_id: "req-123".to_string(),
            success: true,
            results: Some(vec![serde_json::json!({"id": 1})]),
            error: None,
            sender_pseudonym: uuid::Uuid::new_v4().to_string(),
            reply_public_key: "rpk".to_string(),
        };

        let outcome = handle_incoming_query_response(&*store, &payload)
            .await
            .expect("should not error");
        assert!(matches!(outcome, DispatchOutcome::Handled));

        let updated = crate::discovery::async_query::get_async_query(&*store, "req-123")
            .await
            .unwrap()
            .expect("row");
        assert_eq!(updated.status, "completed");
        assert!(updated.results.is_some());
    }

    /// Permanent: query response for an unknown request_id returns
    /// Skipped so the outer dispatcher marks dedup and stops retrying. The
    /// request was pruned (TTL) or was never sent from this node — no
    /// amount of retrying will recover it.
    #[tokio::test]
    async fn query_response_unknown_request_id_is_skipped() {
        let store: Arc<dyn KvStore> = Arc::new(InMemoryKvStore::new());

        let payload = QueryResponsePayload {
            message_type: "query_response".to_string(),
            request_id: "missing-req".to_string(),
            success: true,
            results: Some(vec![]),
            error: None,
            sender_pseudonym: uuid::Uuid::new_v4().to_string(),
            reply_public_key: "rpk".to_string(),
        };

        let outcome = handle_incoming_query_response(&*store, &payload)
            .await
            .expect("should not error");
        match outcome {
            DispatchOutcome::Skipped { reason } => {
                assert!(
                    reason.contains("missing-req"),
                    "reason should name the unknown request_id, got: {reason}"
                );
            }
            DispatchOutcome::Handled => panic!("expected Skipped, got Handled"),
        }
    }

    /// Happy path: schema list response for a known request_id updates
    /// the row and returns Handled.
    #[tokio::test]
    async fn schema_list_response_happy_path_is_handled() {
        let store: Arc<dyn KvStore> = Arc::new(InMemoryKvStore::new());
        seed_async_query(&*store, "req-sl-1", "schema_list").await;

        let payload = SchemaListResponsePayload {
            message_type: "schema_list_response".to_string(),
            request_id: "req-sl-1".to_string(),
            schemas: vec![crate::discovery::async_query::SchemaInfo {
                name: "s1".to_string(),
                descriptive_name: None,
            }],
            sender_pseudonym: uuid::Uuid::new_v4().to_string(),
            reply_public_key: "rpk".to_string(),
        };

        let outcome = handle_incoming_schema_list_response(&*store, &payload)
            .await
            .expect("should not error");
        assert!(matches!(outcome, DispatchOutcome::Handled));

        let updated = crate::discovery::async_query::get_async_query(&*store, "req-sl-1")
            .await
            .unwrap()
            .expect("row");
        assert_eq!(updated.status, "completed");
        assert!(updated.results.is_some());
    }

    /// Permanent: schema list response for an unknown request_id returns
    /// Skipped (dedup marked, no retry forever).
    #[tokio::test]
    async fn schema_list_response_unknown_request_id_is_skipped() {
        let store: Arc<dyn KvStore> = Arc::new(InMemoryKvStore::new());

        let payload = SchemaListResponsePayload {
            message_type: "schema_list_response".to_string(),
            request_id: "missing-sl".to_string(),
            schemas: vec![],
            sender_pseudonym: uuid::Uuid::new_v4().to_string(),
            reply_public_key: "rpk".to_string(),
        };

        let outcome = handle_incoming_schema_list_response(&*store, &payload)
            .await
            .expect("should not error");
        match outcome {
            DispatchOutcome::Skipped { reason } => {
                assert!(
                    reason.contains("missing-sl"),
                    "reason should name the unknown request_id, got: {reason}"
                );
            }
            DispatchOutcome::Handled => panic!("expected Skipped, got Handled"),
        }
    }
}

// ---- Referral match tests ---------------------------------------------------

/// Pure unit tests for [`referral_contact_matches`] / [`referral_voucher_matches`].
///
/// Exercises the stable-identity-pseudonym primary match and the legacy
/// rotating-pseudonym fallback. These pure helpers are the only logic we can
/// test without standing up a full `FoldNode` + publisher, which is precisely
/// why the matching was extracted into them.
#[cfg(test)]
mod referral_match_tests {
    use super::*;
    use chrono::Utc;
    use std::collections::HashMap;

    fn contact_with(
        identity: Option<&str>,
        pseudonym: Option<&str>,
        messaging_pseudonym: Option<&str>,
        messaging_public_key: Option<&str>,
    ) -> Contact {
        Contact {
            public_key: "pk".to_string(),
            display_name: "Bob".to_string(),
            contact_hint: None,
            direction: TrustDirection::Mutual,
            connected_at: Utc::now(),
            pseudonym: pseudonym.map(str::to_string),
            messaging_pseudonym: messaging_pseudonym.map(str::to_string),
            messaging_public_key: messaging_public_key.map(str::to_string),
            identity_pseudonym: identity.map(str::to_string),
            revoked: false,
            roles: HashMap::new(),
        }
    }

    fn query_with(
        subject_identity: Option<&str>,
        subject_pseudonym: &str,
        subject_pk: &str,
    ) -> ReferralQueryPayload {
        ReferralQueryPayload {
            message_type: "referral_query".to_string(),
            query_id: "qid".to_string(),
            subject_pseudonym: subject_pseudonym.to_string(),
            subject_public_key: subject_pk.to_string(),
            sender_pseudonym: "sender".to_string(),
            reply_public_key: "reply".to_string(),
            subject_identity_pseudonym: subject_identity.map(str::to_string),
        }
    }

    fn response_with(
        voucher_identity: Option<&str>,
        sender_pseudonym: &str,
    ) -> ReferralResponsePayload {
        ReferralResponsePayload {
            message_type: "referral_response".to_string(),
            query_id: "qid".to_string(),
            known_as: "Charlie".to_string(),
            sender_pseudonym: sender_pseudonym.to_string(),
            reply_public_key: "reply".to_string(),
            voucher_identity_pseudonym: voucher_identity.map(str::to_string),
        }
    }

    /// **The memory-flagged bug** — rotating pseudonyms differ between Alice
    /// and Bob, so every legacy match field is wrong. The identity pseudonym
    /// is the only key that agrees, and it alone must produce a match.
    #[test]
    fn matches_on_identity_pseudonym_when_all_legacy_fields_mismatch() {
        let bob_contact_row = contact_with(
            Some("IDENTITY-STABLE"),
            Some("bobs-view-of-charlie"),
            Some("bobs-view-of-charlie"),
            Some("bobs-view-pk"),
        );
        // Alice derived totally different rotating values for Charlie:
        let alice_query = query_with(
            Some("IDENTITY-STABLE"),
            "alices-view-of-charlie",
            "alices-view-pk",
        );
        assert!(
            referral_contact_matches(&bob_contact_row, &alice_query),
            "identity pseudonym match must succeed even when every legacy field disagrees"
        );
    }

    /// Legacy compat — an old contact row with no identity pseudonym, and an
    /// old-sender query with no identity pseudonym, must still match via the
    /// rotating pseudonym field. Guards the migration path.
    #[test]
    fn legacy_fallback_matches_on_messaging_pseudonym() {
        let legacy_contact = contact_with(None, None, Some("rotating-pseudo"), Some("old-pk"));
        let legacy_query = query_with(None, "rotating-pseudo", "some-other-pk");
        assert!(referral_contact_matches(&legacy_contact, &legacy_query));
    }

    /// Negative case — both sides have identity pseudonyms but they differ.
    /// The identity match fails and the legacy rotating fields don't agree
    /// either, so the overall match must be false. No silent catch-all.
    #[test]
    fn no_match_when_identity_pseudonyms_differ_and_legacy_fields_disagree() {
        let contact = contact_with(Some("id-a"), Some("rot-a"), Some("rot-a"), Some("pk-a"));
        let query = query_with(Some("id-b"), "rot-b", "pk-b");
        assert!(!referral_contact_matches(&contact, &query));
    }

    /// **Voucher-lookup symptom from the memory doc** — Bob derived
    /// `sender_pseudonym` from the stable `connection-sender` tag, but
    /// Alice's contact row for Bob was keyed off Bob's first opt-in config
    /// name, so the rotating fields disagree. The identity pseudonym is the
    /// one key they share, and it must resolve `voucher_display_name` to
    /// Bob's actual name instead of falling through to "Unknown contact".
    #[test]
    fn voucher_matches_on_identity_pseudonym() {
        let alices_row_for_bob = contact_with(
            Some("BOBS-IDENTITY"),
            Some("alices-view-of-bob"),
            Some("alices-view-of-bob"),
            Some("alices-view-pk"),
        );
        let bobs_response = response_with(Some("BOBS-IDENTITY"), "bobs-connection-sender-pseudo");
        assert!(referral_voucher_matches(
            &alices_row_for_bob,
            &bobs_response
        ));
    }

    #[test]
    fn voucher_legacy_fallback_matches_on_sender_pseudonym() {
        let legacy_row = contact_with(None, None, Some("rotating"), Some("pk"));
        let legacy_response = response_with(None, "rotating");
        assert!(referral_voucher_matches(&legacy_row, &legacy_response));
    }
}

// ---- data_share authorization gate tests -----------------------------------

/// Pure unit tests for the `data_share` trust-boundary gates:
/// [`authorize_data_share_sender`] and [`validate_data_share_schemas`].
///
/// These gates protect against a sender with no existing trust relationship
/// injecting mutations (or inventing schemas) on the recipient's node just
/// by knowing the recipient's messaging pseudonym + pubkey.
#[cfg(test)]
mod data_share_gate_tests {
    use super::*;
    use chrono::Utc;
    use std::collections::HashMap;

    fn active_contact(public_key: &str) -> Contact {
        Contact {
            public_key: public_key.to_string(),
            display_name: "Alice".to_string(),
            contact_hint: None,
            direction: TrustDirection::Mutual,
            connected_at: Utc::now(),
            pseudonym: None,
            messaging_pseudonym: None,
            messaging_public_key: None,
            identity_pseudonym: None,
            revoked: false,
            roles: HashMap::new(),
        }
    }

    fn revoked_contact(public_key: &str) -> Contact {
        let mut c = active_contact(public_key);
        c.revoked = true;
        c
    }

    fn payload_from(sender_pk: &str, schema_names: &[&str]) -> DataSharePayload {
        DataSharePayload {
            message_type: "data_share".to_string(),
            sender_public_key: sender_pk.to_string(),
            sender_display_name: "Alice".to_string(),
            records: schema_names
                .iter()
                .map(|name| SharedRecord {
                    schema_name: (*name).to_string(),
                    schema_definition: None,
                    fields: HashMap::new(),
                    key: SharedRecordKey {
                        hash: Some("h".to_string()),
                        range: None,
                    },
                    file_data_base64: None,
                    file_name: None,
                })
                .collect(),
        }
    }

    #[test]
    fn gate_rejects_unknown_sender() {
        let book = ContactBook::new();
        let payload = payload_from("pk_alice", &["Photography"]);
        assert_eq!(
            authorize_data_share_sender(&book, &payload),
            DataShareAuthz::UnknownSender
        );
    }

    #[test]
    fn gate_rejects_revoked_sender() {
        let mut book = ContactBook::new();
        book.upsert_contact(revoked_contact("pk_alice"));
        let payload = payload_from("pk_alice", &["Photography"]);
        assert_eq!(
            authorize_data_share_sender(&book, &payload),
            DataShareAuthz::RevokedSender
        );
    }

    #[test]
    fn gate_accepts_active_sender() {
        let mut book = ContactBook::new();
        book.upsert_contact(active_contact("pk_alice"));
        let payload = payload_from("pk_alice", &["Photography"]);
        assert_eq!(
            authorize_data_share_sender(&book, &payload),
            DataShareAuthz::Authorized
        );
    }

    #[test]
    fn schema_gate_rejects_unknown_schema() {
        let states: HashMap<String, fold_db::schema::SchemaState> = HashMap::new();
        let payload = payload_from("pk_alice", &["Photography"]);
        assert_eq!(
            validate_data_share_schemas(&states, &payload),
            DataShareSchemaCheck::UnknownOrUnapproved {
                schema_name: "Photography".to_string()
            }
        );
    }

    #[test]
    fn schema_gate_rejects_non_approved_schema() {
        let mut states: HashMap<String, fold_db::schema::SchemaState> = HashMap::new();
        // Present but only Available — not yet approved. Must be rejected to
        // preserve the invariant that users explicitly approve schemas.
        states.insert(
            "Photography".to_string(),
            fold_db::schema::SchemaState::Available,
        );
        let payload = payload_from("pk_alice", &["Photography"]);
        assert_eq!(
            validate_data_share_schemas(&states, &payload),
            DataShareSchemaCheck::UnknownOrUnapproved {
                schema_name: "Photography".to_string()
            }
        );
    }

    #[test]
    fn schema_gate_accepts_all_approved_schemas() {
        let mut states: HashMap<String, fold_db::schema::SchemaState> = HashMap::new();
        states.insert(
            "Photography".to_string(),
            fold_db::schema::SchemaState::Approved,
        );
        states.insert("Travel".to_string(), fold_db::schema::SchemaState::Approved);
        let payload = payload_from("pk_alice", &["Photography", "Travel"]);
        assert_eq!(
            validate_data_share_schemas(&states, &payload),
            DataShareSchemaCheck::AllApproved
        );
    }

    #[test]
    fn schema_gate_rejects_mixed_when_any_is_unknown() {
        let mut states: HashMap<String, fold_db::schema::SchemaState> = HashMap::new();
        states.insert(
            "Photography".to_string(),
            fold_db::schema::SchemaState::Approved,
        );
        // "Travel" is missing entirely — must reject even though the first
        // record is fine. Partial writes are not acceptable.
        let payload = payload_from("pk_alice", &["Photography", "Travel"]);
        assert_eq!(
            validate_data_share_schemas(&states, &payload),
            DataShareSchemaCheck::UnknownOrUnapproved {
                schema_name: "Travel".to_string()
            }
        );
    }
}
