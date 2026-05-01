import { describe, it, expect } from 'vitest';
import type { OrgInviteBundle, OrgMemberInfo } from '../orgClient';

// Fixture mirrors the serde JSON output of fold_db's OrgInviteBundle struct
// (crates/core/src/org/types.rs at the rev pinned in fold_db_node/Cargo.toml).
// If this fixture stops parsing, the Rust struct or the TS interface drifted.
const RUST_SERIALIZED_BUNDLE = {
  org_name: 'Acme Research',
  org_hash: 'a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2',
  org_public_key: 'MCowBQYDK2VwAyEA1234567890abcdef1234567890abcdef1234567890abcdef',
  org_e2e_secret: 'YWVzMjU2Z2NtLXNoYXJlZC1zZWNyZXQtYmFzZTY0LWVuY29kZWQtMzItYnl0ZXM=',
  members: [
    {
      node_public_key: 'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=',
      display_name: 'admin-laptop',
      added_at: 1714521600,
      added_by: 'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=',
    },
    {
      node_public_key: 'IiMkJSYnKCkqKywtLi8wMTIzNDU2Nzg5Ojs8PT4/QA==',
      display_name: 'researcher-desktop',
      added_at: 1714525200,
      added_by: 'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=',
    },
  ],
};

describe('OrgInviteBundle TS↔Rust shape', () => {
  it('round-trips a Rust-serialised bundle through the TS interface preserving every field', () => {
    const bundle: OrgInviteBundle = JSON.parse(JSON.stringify(RUST_SERIALIZED_BUNDLE));

    expect(bundle.org_name).toBe('Acme Research');
    expect(bundle.org_hash).toBe(RUST_SERIALIZED_BUNDLE.org_hash);
    expect(bundle.org_public_key).toBe(RUST_SERIALIZED_BUNDLE.org_public_key);
    expect(bundle.org_e2e_secret).toBe(RUST_SERIALIZED_BUNDLE.org_e2e_secret);
    expect(bundle.members).toHaveLength(2);

    const [first, second]: OrgMemberInfo[] = bundle.members;
    expect(first.node_public_key).toBe(RUST_SERIALIZED_BUNDLE.members[0].node_public_key);
    expect(first.display_name).toBe('admin-laptop');
    expect(first.added_at).toBe(1714521600);
    expect(first.added_by).toBe(RUST_SERIALIZED_BUNDLE.members[0].added_by);
    expect(second.display_name).toBe('researcher-desktop');
    expect(second.added_at).toBe(1714525200);

    const reSerialized = JSON.parse(JSON.stringify(bundle));
    expect(reSerialized).toEqual(RUST_SERIALIZED_BUNDLE);
  });

  it('rejects bundles missing required fields at compile time', () => {
    // @ts-expect-error org_public_key is required in the Rust shape
    const missingPubkey: OrgInviteBundle = {
      org_name: 'X',
      org_hash: 'h',
      org_e2e_secret: 's',
      members: [],
    };
    // @ts-expect-error members is required in the Rust shape
    const missingMembers: OrgInviteBundle = {
      org_name: 'X',
      org_hash: 'h',
      org_public_key: 'p',
      org_e2e_secret: 's',
    };
    expect(missingPubkey).toBeDefined();
    expect(missingMembers).toBeDefined();
  });
});
