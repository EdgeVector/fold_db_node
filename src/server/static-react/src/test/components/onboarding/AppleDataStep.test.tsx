import React from 'react';
import { screen, fireEvent, waitFor } from '@testing-library/react';
import { vi, describe, it, expect, beforeEach, afterEach } from 'vitest';
import AppleDataStep from '../../../components/onboarding/AppleDataStep';
import { renderWithRedux } from '../../utils/testUtilities';

// Default to "all permissions granted" so the silent-failure regression
// tests below exercise the import path the same way they did before the
// pre-flight wiring landed. Tests that exercise the missing-permission
// path override this in their own block.
vi.mock('../../../api/clients/ingestionClient', () => {
  const reject = (msg: string) => () => Promise.reject(new Error(msg));
  return {
    default: {
      getAppleImportStatus: vi.fn().mockResolvedValue({
        success: true,
        data: { available: true },
      }),
      getAppleImportPermissions: vi.fn().mockResolvedValue({
        success: true,
        data: {
          contacts: true,
          notes: true,
          calendar: true,
          reminders: true,
          photos: true,
        },
      }),
      appleImportNotes: vi.fn(reject('notes backend down')),
      appleImportReminders: vi.fn(reject('reminders backend down')),
      appleImportPhotos: vi.fn(reject('photos backend down')),
      appleImportCalendar: vi.fn(reject('calendar backend down')),
      appleImportContacts: vi.fn(reject('contacts backend down')),
      getJobProgress: vi.fn(),
    },
  };
});

import ingestionClient from '../../../api/clients/ingestionClient';

describe('AppleDataStep — silent-failure regression (all sources fail)', () => {
  let onNext: () => void;
  let onSkip: () => void;

  beforeEach(() => {
    onNext = vi.fn();
    onSkip = vi.fn();
  });

  const renderStep = () =>
    renderWithRedux(<AppleDataStep onNext={onNext} onSkip={onSkip} />);

  const clickImport = async () => {
    const btn = await screen.findByRole('button', { name: /Import Selected/i });
    fireEvent.click(btn);
  };

  it('does not show a Continue button when every source fails to start', async () => {
    renderStep();
    await clickImport();

    // Wait for the failure summary to appear, which means handleImportAll
    // has finished iterating through every (rejected) appleImport* call.
    await screen.findByTestId('apple-import-failures');

    // No "Continue" button should be reachable — only Retry / Skip.
    expect(screen.queryByRole('button', { name: /^Continue$/ })).toBeNull();
    expect(screen.getByRole('button', { name: /^Retry$/ })).toBeTruthy();
    expect(screen.getByRole('button', { name: /^Skip$/ })).toBeTruthy();
  });

  it('lists each failed source by label with its error message', async () => {
    renderStep();
    await clickImport();

    const summary = await screen.findByTestId('apple-import-failures');

    // All five source labels should appear inside the failure card.
    const labels = ['Apple Notes', 'Apple Reminders', 'Apple Photos', 'Apple Calendar', 'Apple Contacts'];
    for (const label of labels) {
      expect(summary.textContent).toContain(label);
    }

    // The error messages from the rejected promises should be surfaced too.
    const messages = [
      'notes backend down',
      'reminders backend down',
      'photos backend down',
      'calendar backend down',
      'contacts backend down',
    ];
    for (const msg of messages) {
      expect(summary.textContent).toContain(msg);
    }
  });

  it('Retry re-runs the import and is the only path to leave the failure state', async () => {
    renderStep();
    await clickImport();

    await screen.findByTestId('apple-import-failures');
    const retry = screen.getByRole('button', { name: /^Retry$/ });

    fireEvent.click(retry);

    // After retry, mocks still reject — failures should re-render and there
    // should still be no Continue button.
    await waitFor(() => {
      expect(screen.getByTestId('apple-import-failures')).toBeTruthy();
    });
    expect(screen.queryByRole('button', { name: /^Continue$/ })).toBeNull();
    expect(onNext).not.toHaveBeenCalled();
  });
});

describe('AppleDataStep — Apple permissions pre-flight', () => {
  let onNext: () => void;
  let onSkip: () => void;
  let openSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    onNext = vi.fn();
    onSkip = vi.fn();
    openSpy = vi.spyOn(window, 'open').mockImplementation(() => null);
    // Reset the permissions mock to "all granted" between tests; individual
    // tests override before render. Without this each test would inherit
    // the previous one's stub and silently regress permission gating.
    (ingestionClient.getAppleImportPermissions as ReturnType<typeof vi.fn>)
      .mockReset()
      .mockResolvedValue({
        success: true,
        data: {
          contacts: true,
          notes: true,
          calendar: true,
          reminders: true,
          photos: true,
        },
      });
  });

  afterEach(() => {
    openSpy.mockRestore();
  });

  const renderStep = () =>
    renderWithRedux(<AppleDataStep onNext={onNext} onSkip={onSkip} />);

  it('does NOT render the banner when every selected source has permission', async () => {
    renderStep();
    // Wait for the toggle list to appear (post-status, post-permissions).
    await screen.findByRole('button', { name: /Import Selected/i });
    expect(screen.queryByTestId('apple-permissions-banner')).toBeNull();
  });

  it('renders the banner naming the missing source(s) when a probe returns false', async () => {
    (ingestionClient.getAppleImportPermissions as ReturnType<typeof vi.fn>)
      .mockReset()
      .mockResolvedValue({
        success: true,
        data: {
          contacts: false,
          notes: true,
          calendar: true,
          reminders: true,
          photos: true,
        },
      });

    renderStep();
    const banner = await screen.findByTestId('apple-permissions-banner');
    expect(banner.textContent).toContain('Apple Contacts');
    expect(banner.textContent).toContain('System Settings');
    expect(banner.textContent).toContain('Automation');
  });

  it('the banner only counts SELECTED sources — deselecting the missing source dismisses it', async () => {
    (ingestionClient.getAppleImportPermissions as ReturnType<typeof vi.fn>)
      .mockReset()
      .mockResolvedValue({
        success: true,
        data: {
          contacts: false,
          notes: true,
          calendar: true,
          reminders: true,
          photos: true,
        },
      });

    renderStep();
    await screen.findByTestId('apple-permissions-banner');

    // Find the Contacts toggle and uncheck it. Without the SELECTED check,
    // a stale `permissions` map would keep showing the banner even after
    // the user opted out — that's the regression to guard.
    const contactsToggle = screen.getByLabelText(/Apple Contacts/i) as HTMLInputElement;
    fireEvent.click(contactsToggle);

    await waitFor(() => {
      expect(screen.queryByTestId('apple-permissions-banner')).toBeNull();
    });
  });

  it('clicking "Open System Settings" launches the Privacy_Automation deep link', async () => {
    (ingestionClient.getAppleImportPermissions as ReturnType<typeof vi.fn>)
      .mockReset()
      .mockResolvedValue({
        success: true,
        data: {
          contacts: false,
          notes: true,
          calendar: true,
          reminders: true,
          photos: true,
        },
      });

    renderStep();
    await screen.findByTestId('apple-permissions-banner');

    fireEvent.click(screen.getByRole('button', { name: /Open System Settings/i }));
    expect(openSpy).toHaveBeenCalledWith(
      expect.stringContaining('x-apple.systempreferences:com.apple.preference.security?Privacy_Automation'),
      expect.anything(),
    );
  });

  it('"Import Selected" re-checks permissions and bails BEFORE spawning any imports if anything is still missing', async () => {
    (ingestionClient.getAppleImportPermissions as ReturnType<typeof vi.fn>)
      .mockReset()
      .mockResolvedValue({
        success: true,
        data: {
          contacts: false,
          notes: true,
          calendar: true,
          reminders: true,
          photos: true,
        },
      });

    renderStep();
    await screen.findByTestId('apple-permissions-banner');

    fireEvent.click(screen.getByRole('button', { name: /Import Selected/i }));

    // The pre-flight should have re-fired and the import path must NOT have
    // been entered. Specifically: no progress card, no failure card, no
    // background imports kicked off.
    await waitFor(() => {
      expect(
        (ingestionClient.getAppleImportPermissions as ReturnType<typeof vi.fn>).mock.calls
          .length,
      ).toBeGreaterThanOrEqual(2); // mount + click
    });
    expect(screen.queryByTestId('apple-import-failures')).toBeNull();
    expect(ingestionClient.appleImportContacts).not.toHaveBeenCalled();
    expect(ingestionClient.appleImportNotes).not.toHaveBeenCalled();
  });

  it('renders the imported count from the structured results payload, not by parsing status_message', async () => {
    // Pins the bug fix on the React side: when the backend completes an
    // apple-import job it now publishes `results: {source, total, ingested}`
    // on the progress payload. The wizard must read that structured field
    // (and not regex `"Imported {N} notes"` out of `status_message`).
    const ingestionMock = ingestionClient as unknown as {
      appleImportNotes: ReturnType<typeof vi.fn>;
      appleImportReminders: ReturnType<typeof vi.fn>;
      appleImportPhotos: ReturnType<typeof vi.fn>;
      appleImportCalendar: ReturnType<typeof vi.fn>;
      appleImportContacts: ReturnType<typeof vi.fn>;
      getJobProgress: ReturnType<typeof vi.fn>;
    };
    ingestionMock.appleImportNotes
      .mockReset()
      .mockResolvedValue({ success: true, data: { progress_id: 'pid-notes' } });
    ingestionMock.appleImportReminders
      .mockReset()
      .mockResolvedValue({ success: true, data: { progress_id: 'pid-reminders' } });
    ingestionMock.appleImportPhotos
      .mockReset()
      .mockResolvedValue({ success: true, data: { progress_id: 'pid-photos' } });
    ingestionMock.appleImportCalendar
      .mockReset()
      .mockResolvedValue({ success: true, data: { progress_id: 'pid-calendar' } });
    ingestionMock.appleImportContacts
      .mockReset()
      .mockResolvedValue({ success: true, data: { progress_id: 'pid-contacts' } });

    const completedFor = (source: string, total: number, ingested: number) => ({
      success: true,
      data: {
        progress_percentage: 100,
        status_message: `Imported ${ingested} ${source.replace('apple-', '')}`,
        is_complete: true,
        is_failed: false,
        results: { source, total, ingested },
      },
    });

    ingestionMock.getJobProgress
      .mockReset()
      .mockImplementation((pid: string) => {
        switch (pid) {
          case 'pid-notes':
            return Promise.resolve(completedFor('apple-notes', 132, 132));
          case 'pid-reminders':
            return Promise.resolve(completedFor('apple-reminders', 4, 4));
          case 'pid-photos':
            return Promise.resolve(completedFor('apple-photos', 50, 50));
          case 'pid-calendar':
            return Promise.resolve(completedFor('apple-calendar', 12, 12));
          case 'pid-contacts':
            return Promise.resolve(completedFor('apple-contacts', 220, 219));
          default:
            return Promise.resolve({ success: false });
        }
      });

    renderStep();
    fireEvent.click(await screen.findByRole('button', { name: /Import Selected/i }));

    await waitFor(() => {
      const counts = screen.getAllByTestId('apple-import-count');
      expect(counts.length).toBe(5);
    });

    const counts = screen.getAllByTestId('apple-import-count').map((n) => n.textContent);
    expect(counts).toEqual(
      expect.arrayContaining([
        expect.stringContaining('132 imported of 132'),
        expect.stringContaining('4 imported of 4'),
        expect.stringContaining('50 imported of 50'),
        expect.stringContaining('12 imported of 12'),
        expect.stringContaining('219 imported of 220'),
      ]),
    );
  });

  it('after the user grants access and clicks Refresh, the banner disappears', async () => {
    const mock = ingestionClient.getAppleImportPermissions as ReturnType<typeof vi.fn>;
    mock.mockReset()
      .mockResolvedValueOnce({
        success: true,
        data: {
          contacts: false,
          notes: true,
          calendar: true,
          reminders: true,
          photos: true,
        },
      })
      .mockResolvedValueOnce({
        success: true,
        data: {
          contacts: true,
          notes: true,
          calendar: true,
          reminders: true,
          photos: true,
        },
      });

    renderStep();
    await screen.findByTestId('apple-permissions-banner');

    fireEvent.click(screen.getByTestId('apple-permissions-refresh'));
    await waitFor(() => {
      expect(screen.queryByTestId('apple-permissions-banner')).toBeNull();
    });
  });
});
