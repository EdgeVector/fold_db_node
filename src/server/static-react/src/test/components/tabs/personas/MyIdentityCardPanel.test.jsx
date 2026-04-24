import React from 'react'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import MyIdentityCardPanel from '../../../../components/tabs/personas/MyIdentityCardPanel'

vi.mock('../../../../api/clients/fingerprintsClient', () => ({
  getMyIdentityCard: vi.fn(),
  reissueMyIdentityCard: vi.fn(),
  sendIdentityCard: vi.fn(),
  detectFaces: vi.fn(),
}))
vi.mock('../../../../api/clients/trustClient', () => ({
  listContacts: vi.fn(),
}))

import {
  detectFaces,
  getMyIdentityCard,
  reissueMyIdentityCard,
  sendIdentityCard,
} from '../../../../api/clients/fingerprintsClient'
import { listContacts } from '../../../../api/clients/trustClient'

function card(overrides = {}) {
  return {
    pub_key: 'pk_abc',
    display_name: 'Tom Tang',
    birthday: '1990-04-17',
    face_embedding: null,
    node_id: 'pk_abc',
    card_signature:
      'sig_base64_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
    issued_at: '2026-04-14T12:00:00Z',
    ...overrides,
  }
}

function ok(data) {
  return { success: true, data }
}

describe('MyIdentityCardPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('renders the card fields when present', async () => {
    getMyIdentityCard.mockResolvedValue(ok(card()))
    render(<MyIdentityCardPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('my-identity-card-fields')).toBeInTheDocument()
    })
    const fields = screen.getByTestId('my-identity-card-fields')
    expect(fields).toHaveTextContent('Tom Tang')
    expect(fields).toHaveTextContent('1990-04-17')
    expect(fields).toHaveTextContent('pk_abc')
    expect(fields).toHaveTextContent('2026-04-14T12:00:00Z')
  })

  it('shows "not set" when birthday is null', async () => {
    getMyIdentityCard.mockResolvedValue(ok(card({ birthday: null })))
    render(<MyIdentityCardPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('my-identity-card-fields')).toBeInTheDocument()
    })
    expect(screen.getByTestId('my-identity-card-fields')).toHaveTextContent(
      'not set',
    )
  })

  it('surfaces the backend error (e.g. 404 when no card yet)', async () => {
    getMyIdentityCard.mockResolvedValue({
      success: false,
      error: 'self-Identity not yet issued — complete the setup wizard first',
    })
    render(<MyIdentityCardPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('my-identity-card-error')).toHaveTextContent(
        'not yet issued',
      )
    })
  })

  it('copies the card JSON to the clipboard on Copy click', async () => {
    getMyIdentityCard.mockResolvedValue(ok(card()))
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText },
      configurable: true,
    })

    render(<MyIdentityCardPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('my-identity-card-copy')).toBeInTheDocument()
    })
    fireEvent.click(screen.getByTestId('my-identity-card-copy'))
    await waitFor(() => {
      expect(writeText).toHaveBeenCalledTimes(1)
    })
    const payload = writeText.mock.calls[0][0]
    // Payload is canonical JSON including every field.
    const parsed = JSON.parse(payload)
    expect(parsed.pub_key).toBe('pk_abc')
    expect(parsed.display_name).toBe('Tom Tang')
    expect(parsed.card_signature).toMatch(/^sig_/)
  })

  it('renders the pretty-printed JSON below the fields', async () => {
    getMyIdentityCard.mockResolvedValue(ok(card()))
    render(<MyIdentityCardPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('my-identity-card-json')).toBeInTheDocument()
    })
    expect(screen.getByTestId('my-identity-card-json')).toHaveTextContent(
      '"display_name": "Tom Tang"',
    )
  })

  describe('edit + reissue flow', () => {
    it('shows the edit form after clicking Edit card', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-edit')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-edit'))
      expect(screen.getByTestId('my-identity-card-edit-form')).toBeInTheDocument()
      expect(screen.getByTestId('my-identity-card-draft-name')).toHaveValue(
        'Tom Tang',
      )
      // Birthday is intentionally NOT editable here — a real person's
      // birthday doesn't change; surfacing it would imply otherwise.
      expect(
        screen.queryByTestId('my-identity-card-draft-birthday'),
      ).toBeNull()
    })

    it('calls reissueMyIdentityCard with only the changed display_name', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      reissueMyIdentityCard.mockResolvedValue(
        ok(card({ display_name: 'Thomas', issued_at: '2026-04-18T00:00:00Z' })),
      )
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-edit')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-edit'))
      fireEvent.change(screen.getByTestId('my-identity-card-draft-name'), {
        target: { value: 'Thomas' },
      })
      fireEvent.click(screen.getByTestId('my-identity-card-save'))
      await waitFor(() => {
        expect(reissueMyIdentityCard).toHaveBeenCalledWith({
          display_name: 'Thomas',
        })
      })
      // After success we drop back to the display view with the new card.
      await waitFor(() => {
        expect(screen.queryByTestId('my-identity-card-edit-form')).toBeNull()
      })
      expect(screen.getByTestId('my-identity-card-fields')).toHaveTextContent(
        'Thomas',
      )
    })

    it('does not call reissue when nothing changed', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-edit')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-edit'))
      fireEvent.click(screen.getByTestId('my-identity-card-save'))
      expect(reissueMyIdentityCard).not.toHaveBeenCalled()
      // Form closes even though no network call happened.
      expect(screen.queryByTestId('my-identity-card-edit-form')).toBeNull()
    })

    it('surfaces backend error messages in the save-error slot', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      reissueMyIdentityCard.mockResolvedValue({
        success: false,
        error: 'display_name must not be empty',
      })
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-edit')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-edit'))
      fireEvent.change(screen.getByTestId('my-identity-card-draft-name'), {
        target: { value: 'Rename' },
      })
      fireEvent.click(screen.getByTestId('my-identity-card-save'))
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-save-error'),
        ).toHaveTextContent(/display_name must not be empty/)
      })
      // Form stays open so the user can fix the input.
      expect(screen.getByTestId('my-identity-card-edit-form')).toBeInTheDocument()
    })

    it('Cancel restores the display view without reissuing', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-edit')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-edit'))
      fireEvent.change(screen.getByTestId('my-identity-card-draft-name'), {
        target: { value: 'Rename' },
      })
      fireEvent.click(screen.getByTestId('my-identity-card-cancel'))
      expect(reissueMyIdentityCard).not.toHaveBeenCalled()
      expect(screen.queryByTestId('my-identity-card-edit-form')).toBeNull()
    })
  })

  describe('QR render', () => {
    it('hides the QR block by default', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-qr-toggle')).toBeInTheDocument()
      })
      expect(screen.queryByTestId('my-identity-card-qr')).toBeNull()
    })

    it('renders the QR SVG when Show QR is clicked', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-qr-toggle')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-qr-toggle'))
      expect(screen.getByTestId('my-identity-card-qr')).toBeInTheDocument()
      expect(screen.getByTestId('my-identity-card-qr-toggle')).toHaveAttribute(
        'aria-pressed',
        'true',
      )
    })

    it('toggles back off on a second click', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-qr-toggle')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-qr-toggle'))
      fireEvent.click(screen.getByTestId('my-identity-card-qr-toggle'))
      expect(screen.queryByTestId('my-identity-card-qr')).toBeNull()
    })
  })

  describe('QR export (TODO-9)', () => {
    // Capture every blob passed to URL.createObjectURL so the
    // download assertions can inspect what the handler tried to
    // save. Cleared per-test via beforeEach's vi.clearAllMocks.
    let createdBlobs
    let anchorClicks
    let origCreateObjectURL
    let origRevokeObjectURL

    beforeEach(() => {
      createdBlobs = []
      anchorClicks = []
      // jsdom does not implement these by default; stub them so the
      // handlers run to completion without blowing up on missing
      // browser primitives. Stash the originals so the afterEach can
      // restore them (vi.restoreAllMocks doesn't touch direct
      // assignments on URL).
      origCreateObjectURL = globalThis.URL.createObjectURL
      origRevokeObjectURL = globalThis.URL.revokeObjectURL
      globalThis.URL.createObjectURL = vi.fn(blob => {
        createdBlobs.push(blob)
        return `blob:mock:${createdBlobs.length}`
      })
      globalThis.URL.revokeObjectURL = vi.fn()
      // Intercept anchor clicks on the prototype so we observe the
      // download filename without touching document.createElement
      // (spying on that recursed infinitely across test files).
      vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(
        function clickSpy() {
          anchorClicks.push({ href: this.href, download: this.download })
        },
      )
    })

    afterEach(() => {
      globalThis.URL.createObjectURL = origCreateObjectURL
      globalThis.URL.revokeObjectURL = origRevokeObjectURL
      vi.restoreAllMocks()
    })

    it('exposes Download PNG and Download SVG buttons once QR is open', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-qr-toggle')).toBeInTheDocument()
      })
      // Hidden while the QR panel is collapsed.
      expect(
        screen.queryByTestId('my-identity-card-qr-download-png'),
      ).toBeNull()
      expect(
        screen.queryByTestId('my-identity-card-qr-download-svg'),
      ).toBeNull()

      fireEvent.click(screen.getByTestId('my-identity-card-qr-toggle'))

      expect(
        screen.getByTestId('my-identity-card-qr-download-png'),
      ).toBeInTheDocument()
      expect(
        screen.getByTestId('my-identity-card-qr-download-svg'),
      ).toBeInTheDocument()
    })

    it('downloads an SVG file named after the card owner', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card({ display_name: 'Tom Tang' })))
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-qr-toggle')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-qr-toggle'))
      fireEvent.click(screen.getByTestId('my-identity-card-qr-download-svg'))

      expect(anchorClicks).toHaveLength(1)
      expect(anchorClicks[0].download).toBe('identity-tom-tang.svg')
      expect(createdBlobs).toHaveLength(1)
      expect(createdBlobs[0].type).toContain('image/svg+xml')
    })

    it('falls back to a generic filename when display_name is empty', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card({ display_name: '' })))
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-qr-toggle')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-qr-toggle'))
      fireEvent.click(screen.getByTestId('my-identity-card-qr-download-svg'))

      expect(anchorClicks[0].download).toBe('identity-card.svg')
    })

    it('sanitizes punctuation and case in the display name', async () => {
      getMyIdentityCard.mockResolvedValue(
        ok(card({ display_name: 'O\u2019Brien & Co.' })),
      )
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-qr-toggle')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-qr-toggle'))
      fireEvent.click(screen.getByTestId('my-identity-card-qr-download-svg'))

      expect(anchorClicks[0].download).toBe('identity-o-brien-co.svg')
    })

    it('PNG button starts the rasterize pipeline by creating an SVG blob', async () => {
      // jsdom does not fire Image.onload under vitest, so we can't
      // observe the final PNG blob — but we can verify that clicking
      // Download PNG immediately produced an SVG blob URL and set it
      // as the Image src, which is the handler's first side effect.
      getMyIdentityCard.mockResolvedValue(ok(card()))
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-qr-toggle')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-qr-toggle'))
      fireEvent.click(screen.getByTestId('my-identity-card-qr-download-png'))

      expect(createdBlobs).toHaveLength(1)
      expect(createdBlobs[0].type).toContain('image/svg+xml')
      // No anchor click yet — that only fires after Image.onload +
      // canvas.toBlob, which won't run in this environment.
      expect(anchorClicks).toHaveLength(0)
    })
  })

  describe('send to contact', () => {
    const aliceContact = {
      public_key: 'pk_alice',
      display_name: 'Alice',
      direction: 'mutual',
      connected_at: '2026-04-01T00:00:00Z',
      revoked: false,
    }
    const bobContact = {
      public_key: 'pk_bob',
      display_name: 'Bob',
      direction: 'mutual',
      connected_at: '2026-04-02T00:00:00Z',
      revoked: false,
    }
    const revokedContact = {
      public_key: 'pk_revoked',
      display_name: 'Gone',
      direction: 'mutual',
      connected_at: '2026-03-01T00:00:00Z',
      revoked: true,
    }

    it('does not open the picker by default and does not fetch contacts', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-send')).toBeInTheDocument()
      })
      expect(listContacts).not.toHaveBeenCalled()
      expect(screen.queryByTestId('my-identity-card-send-picker')).toBeNull()
    })

    it('opens the picker and loads contacts on first click', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      listContacts.mockResolvedValue(
        ok({ contacts: [aliceContact, bobContact] }),
      )
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-send')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-send'))
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-send-picker'),
        ).toBeInTheDocument()
      })
      expect(listContacts).toHaveBeenCalledTimes(1)
      expect(
        screen.getByTestId('my-identity-card-send-to-pk_alice'),
      ).toBeInTheDocument()
      expect(
        screen.getByTestId('my-identity-card-send-to-pk_bob'),
      ).toBeInTheDocument()
    })

    it('filters revoked contacts out of the send list', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      listContacts.mockResolvedValue(
        ok({ contacts: [aliceContact, revokedContact] }),
      )
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-send')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-send'))
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-send-to-pk_alice'),
        ).toBeInTheDocument()
      })
      expect(
        screen.queryByTestId('my-identity-card-send-to-pk_revoked'),
      ).toBeNull()
    })

    it('shows the empty-state message when the user has no contacts', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      listContacts.mockResolvedValue(ok({ contacts: [] }))
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-send')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-send'))
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-send-empty'),
        ).toBeInTheDocument()
      })
    })

    it('calls sendIdentityCard(pub_key) when a contact row is clicked', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      listContacts.mockResolvedValue(ok({ contacts: [aliceContact] }))
      sendIdentityCard.mockResolvedValue(
        ok({
          message_id: 'msg_abc',
          recipient_display_name: 'Alice',
        }),
      )
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-send')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-send'))
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-send-to-pk_alice'),
        ).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-send-to-pk_alice'))
      await waitFor(() => {
        expect(sendIdentityCard).toHaveBeenCalledWith('pk_alice')
      })
      // Picker closes on success and a confirmation line appears.
      await waitFor(() => {
        expect(
          screen.queryByTestId('my-identity-card-send-picker'),
        ).toBeNull()
      })
      expect(screen.getByTestId('my-identity-card-send-result')).toHaveTextContent(
        /Sent to Alice/,
      )
    })

    it('surfaces backend errors in the picker without closing it', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      listContacts.mockResolvedValue(ok({ contacts: [aliceContact] }))
      sendIdentityCard.mockResolvedValue({
        success: false,
        error: 'Contact does not have messaging enabled. Connect via discovery first.',
      })
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-send')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-send'))
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-send-to-pk_alice'),
        ).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-send-to-pk_alice'))
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-send-error'),
        ).toHaveTextContent(/messaging enabled/)
      })
      // Picker stays open so the user can try a different contact.
      expect(
        screen.getByTestId('my-identity-card-send-picker'),
      ).toBeInTheDocument()
    })
  })

  describe('attach / remove face', () => {
    function stubGetUserMedia(stream = { getTracks: () => [] }) {
      const getUserMedia = vi.fn().mockResolvedValue(stream)
      Object.defineProperty(navigator, 'mediaDevices', {
        value: { getUserMedia },
        configurable: true,
      })
      return getUserMedia
    }

    function stubToDataURL() {
      // jsdom doesn't implement canvas. Patch HTMLCanvasElement so
      // getContext/toDataURL don't blow up — we don't actually care
      // about the pixel data because detectFaces is mocked.
      HTMLCanvasElement.prototype.getContext = vi.fn().mockReturnValue({
        drawImage: vi.fn(),
      })
      HTMLCanvasElement.prototype.toDataURL = vi
        .fn()
        .mockReturnValue('data:image/png;base64,AAAA')
    }

    it('hides the Attach button until the card is loaded', async () => {
      // Resolve later so the panel stays in the loading branch.
      getMyIdentityCard.mockReturnValue(new Promise(() => {}))
      render(<MyIdentityCardPanel />)
      expect(
        screen.queryByTestId('my-identity-card-attach-face'),
      ).toBeNull()
    })

    it('detects a single face and calls reissue with the embedding', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card({ face_embedding: null })))
      const getUserMedia = stubGetUserMedia()
      stubToDataURL()
      detectFaces.mockResolvedValue(
        ok({
          faces: [
            {
              embedding: [0.1, 0.2, 0.3],
              bbox: [0, 0, 1, 1],
              confidence: 0.95,
            },
          ],
        }),
      )
      reissueMyIdentityCard.mockResolvedValue(
        ok(card({ face_embedding: [0.1, 0.2, 0.3] })),
      )
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-attach-face'),
        ).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-attach-face'))
      await waitFor(() => {
        expect(getUserMedia).toHaveBeenCalledTimes(1)
      })
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-attach-snap'),
        ).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-attach-snap'))
      await waitFor(() => {
        expect(reissueMyIdentityCard).toHaveBeenCalledWith({
          face_embedding: [0.1, 0.2, 0.3],
        })
      })
      // Modal closes on success.
      await waitFor(() => {
        expect(
          screen.queryByTestId('my-identity-card-attach-modal'),
        ).toBeNull()
      })
    })

    it('shows a "no face detected" error when detect returns 0 faces', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card({ face_embedding: null })))
      stubGetUserMedia()
      stubToDataURL()
      detectFaces.mockResolvedValue(ok({ faces: [] }))
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-attach-face'),
        ).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-attach-face'))
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-attach-snap'),
        ).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-attach-snap'))
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-attach-error'),
        ).toHaveTextContent(/No face detected/)
      })
      expect(reissueMyIdentityCard).not.toHaveBeenCalled()
      expect(
        screen.getByTestId('my-identity-card-attach-modal'),
      ).toBeInTheDocument()
    })

    it('shows a "multiple faces" error when detect returns 2+ faces', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card({ face_embedding: null })))
      stubGetUserMedia()
      stubToDataURL()
      detectFaces.mockResolvedValue(
        ok({
          faces: [
            { embedding: [0.1], bbox: [0, 0, 1, 1], confidence: 0.9 },
            { embedding: [0.2], bbox: [1, 1, 2, 2], confidence: 0.9 },
          ],
        }),
      )
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-attach-face'),
        ).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-attach-face'))
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-attach-snap'),
        ).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-attach-snap'))
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-attach-error'),
        ).toHaveTextContent(/Multiple faces detected/)
      })
      expect(reissueMyIdentityCard).not.toHaveBeenCalled()
    })

    it('Remove face calls reissue with face_embedding: null', async () => {
      getMyIdentityCard.mockResolvedValue(
        ok(card({ face_embedding: [0.1, 0.2, 0.3] })),
      )
      reissueMyIdentityCard.mockResolvedValue(
        ok(card({ face_embedding: null })),
      )
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-remove-face'),
        ).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-remove-face'))
      await waitFor(() => {
        expect(reissueMyIdentityCard).toHaveBeenCalledWith({
          face_embedding: null,
        })
      })
      // After success the button flips back to Attach.
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-attach-face'),
        ).toBeInTheDocument()
      })
    })
  })
})
