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
