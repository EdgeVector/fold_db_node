import React from 'react'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { describe, it, expect, beforeEach, vi } from 'vitest'
import CrossUserSharingPanel from '../../../../components/tabs/sharing/CrossUserSharingPanel'

vi.mock('../../../../api/clients/sharingClient', () => ({
  listShareRules: vi.fn(),
  createShareRule: vi.fn(),
  deactivateShareRule: vi.fn(),
  generateShareInvite: vi.fn(),
  acceptShareInvite: vi.fn(),
  listPendingShareInvites: vi.fn(),
}))

vi.mock('../../../../api/clients/trustClient', () => ({
  listContacts: vi.fn(),
}))

vi.mock('../../../../api/clients/schemaClient', () => ({
  getAllSchemasWithState: vi.fn(),
}))

import {
  listShareRules,
  createShareRule,
  deactivateShareRule,
  generateShareInvite,
  acceptShareInvite,
  listPendingShareInvites,
} from '../../../../api/clients/sharingClient'
import { listContacts } from '../../../../api/clients/trustClient'
import { getAllSchemasWithState } from '../../../../api/clients/schemaClient'

function ok(data) {
  return { success: true, data }
}

function makeRule(overrides = {}) {
  return {
    rule_id: 'rule_default',
    recipient_pubkey: 'aa'.repeat(32),
    recipient_display_name: 'Alice',
    scope: 'AllSchemas',
    share_prefix: 'share:660e03bf:876a4905',
    share_e2e_secret: new Array(32).fill(0),
    active: true,
    created_at: 1_700_000_000,
    writer_pubkey: 'bb'.repeat(32),
    signature: 'deadbeef'.repeat(8),
    ...overrides,
  }
}

function makeInvite(overrides = {}) {
  return {
    sender_pubkey: 'cc'.repeat(32),
    sender_display_name: 'Bob',
    share_prefix: 'share:abc:def',
    share_e2e_secret: new Array(32).fill(0),
    scope_description: 'Schema: Notes',
    ...overrides,
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  listContacts.mockResolvedValue(ok({ contacts: [] }))
  getAllSchemasWithState.mockResolvedValue(ok({ Notes: 'Approved' }))
  // Polyfill the clipboard API for JSDOM
  if (!navigator.clipboard) {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
    })
  }
})

describe('CrossUserSharingPanel', () => {
  it('renders empty state when there are no rules or invites', async () => {
    listShareRules.mockResolvedValue(ok({ ok: true, rules: [] }))
    listPendingShareInvites.mockResolvedValue(ok({ ok: true, invites: [] }))

    render(<CrossUserSharingPanel />)

    await waitFor(() => {
      expect(screen.getByTestId('my-rules-empty')).toBeInTheDocument()
      expect(screen.getByTestId('pending-invites-empty')).toBeInTheDocument()
    })
  })

  it('renders a list of rules with signed badge and scope', async () => {
    listShareRules.mockResolvedValue(
      ok({
        ok: true,
        rules: [
          makeRule({ rule_id: 'r1', recipient_display_name: 'Alice' }),
          makeRule({
            rule_id: 'r2',
            recipient_display_name: 'Carol',
            scope: { Schema: 'Notes' },
          }),
        ],
      }),
    )
    listPendingShareInvites.mockResolvedValue(ok({ ok: true, invites: [] }))

    render(<CrossUserSharingPanel />)

    await waitFor(() => {
      expect(screen.getByTestId('rule-row-r1')).toBeInTheDocument()
    })
    expect(screen.getByTestId('rule-row-r1')).toHaveTextContent('Alice')
    expect(screen.getByTestId('rule-row-r1')).toHaveTextContent('All my data')
    expect(screen.getByTestId('rule-signed-r1')).toHaveTextContent('Signed')

    expect(screen.getByTestId('rule-row-r2')).toHaveTextContent('Carol')
    expect(screen.getByTestId('rule-row-r2')).toHaveTextContent('Schema: Notes')
  })

  it('submits the correct payload when creating a rule', async () => {
    listShareRules.mockResolvedValue(ok({ ok: true, rules: [] }))
    listPendingShareInvites.mockResolvedValue(ok({ ok: true, invites: [] }))
    createShareRule.mockResolvedValue(
      ok({ ok: true, rule: makeRule({ rule_id: 'r_new' }) }),
    )

    render(<CrossUserSharingPanel />)

    await waitFor(() => {
      expect(screen.getByTestId('create-rule-form')).toBeInTheDocument()
    })

    fireEvent.change(screen.getByTestId('create-rule-pubkey'), {
      target: { value: 'aa'.repeat(32) },
    })
    fireEvent.change(screen.getByTestId('create-rule-name'), {
      target: { value: 'Alice' },
    })
    // Default scope is AllSchemas, no schema needed
    fireEvent.click(screen.getByTestId('create-rule-submit'))

    await waitFor(() => {
      expect(createShareRule).toHaveBeenCalledWith({
        recipient_pubkey: 'aa'.repeat(32),
        recipient_display_name: 'Alice',
        scope: 'AllSchemas',
      })
    })
  })

  it('calls deactivateShareRule with the correct rule id', async () => {
    // jsdom provides window.confirm — force it true so the flow proceeds
    vi.spyOn(window, 'confirm').mockReturnValue(true)

    listShareRules.mockResolvedValue(
      ok({ ok: true, rules: [makeRule({ rule_id: 'r_del' })] }),
    )
    listPendingShareInvites.mockResolvedValue(ok({ ok: true, invites: [] }))
    deactivateShareRule.mockResolvedValue(ok({ ok: true }))

    render(<CrossUserSharingPanel />)

    await waitFor(() => {
      expect(screen.getByTestId('rule-deactivate-r_del')).toBeInTheDocument()
    })
    fireEvent.click(screen.getByTestId('rule-deactivate-r_del'))

    await waitFor(() => {
      expect(deactivateShareRule).toHaveBeenCalledWith('r_del')
    })
  })

  it('calls generateShareInvite with the rule id and a scope description', async () => {
    listShareRules.mockResolvedValue(
      ok({
        ok: true,
        rules: [makeRule({ rule_id: 'r_inv', scope: { Schema: 'Notes' } })],
      }),
    )
    listPendingShareInvites.mockResolvedValue(ok({ ok: true, invites: [] }))
    generateShareInvite.mockResolvedValue(
      ok({ ok: true, invite: makeInvite() }),
    )

    render(<CrossUserSharingPanel />)

    await waitFor(() => {
      expect(screen.getByTestId('rule-invite-r_inv')).toBeInTheDocument()
    })
    fireEvent.click(screen.getByTestId('rule-invite-r_inv'))

    await waitFor(() => {
      expect(generateShareInvite).toHaveBeenCalledWith({
        rule_id: 'r_inv',
        scope_description: 'Schema: Notes',
      })
    })
  })

  it('displays pending invites with sender and scope', async () => {
    listShareRules.mockResolvedValue(ok({ ok: true, rules: [] }))
    listPendingShareInvites.mockResolvedValue(
      ok({
        ok: true,
        invites: [
          makeInvite({
            share_prefix: 'share:aaa:bbb',
            sender_display_name: 'Bob',
            scope_description: 'All my data',
          }),
        ],
      }),
    )

    render(<CrossUserSharingPanel />)

    await waitFor(() => {
      expect(
        screen.getByTestId('pending-invite-row-share:aaa:bbb'),
      ).toBeInTheDocument()
    })
    expect(screen.getByTestId('pending-invite-row-share:aaa:bbb')).toHaveTextContent(
      'Bob',
    )
    expect(screen.getByTestId('pending-invite-row-share:aaa:bbb')).toHaveTextContent(
      'All my data',
    )
  })

  it('removes an invite from the list after Accept succeeds', async () => {
    listShareRules.mockResolvedValue(ok({ ok: true, rules: [] }))
    listPendingShareInvites.mockResolvedValue(
      ok({
        ok: true,
        invites: [makeInvite({ share_prefix: 'share:xyz:www' })],
      }),
    )
    acceptShareInvite.mockResolvedValue(
      ok({
        ok: true,
        subscription: {
          sender_pubkey: 'cc'.repeat(32),
          share_prefix: 'share:xyz:www',
          share_e2e_secret: new Array(32).fill(0),
          accepted_at: 1_700_000_000,
          active: true,
        },
      }),
    )

    render(<CrossUserSharingPanel />)

    await waitFor(() => {
      expect(
        screen.getByTestId('pending-invite-accept-share:xyz:www'),
      ).toBeInTheDocument()
    })
    fireEvent.click(screen.getByTestId('pending-invite-accept-share:xyz:www'))

    await waitFor(() => {
      expect(acceptShareInvite).toHaveBeenCalled()
      expect(
        screen.queryByTestId('pending-invite-row-share:xyz:www'),
      ).toBeNull()
    })
  })
})
