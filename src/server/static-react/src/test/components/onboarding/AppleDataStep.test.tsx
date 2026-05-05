import React from 'react';
import { screen, fireEvent, waitFor } from '@testing-library/react';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import AppleDataStep from '../../../components/onboarding/AppleDataStep';
import { renderWithRedux } from '../../utils/testUtilities';

vi.mock('../../../api/clients/ingestionClient', () => {
  const reject = (msg: string) => () => Promise.reject(new Error(msg));
  return {
    default: {
      getAppleImportStatus: vi.fn().mockResolvedValue({
        success: true,
        data: { available: true },
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
