import React from 'react';
import { screen, fireEvent, waitFor } from '@testing-library/react';
import { vi, describe, it, expect, beforeEach, afterEach } from 'vitest';
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

    await screen.findByTestId('apple-import-failures');

    expect(screen.queryByRole('button', { name: /^Continue$/ })).toBeNull();
    expect(screen.getByRole('button', { name: /^Retry$/ })).toBeTruthy();
    expect(screen.getByRole('button', { name: /^Skip$/ })).toBeTruthy();
  });

  it('lists each failed source by label with its error message', async () => {
    renderStep();
    await clickImport();

    const summary = await screen.findByTestId('apple-import-failures');

    const labels = ['Apple Notes', 'Apple Reminders', 'Apple Photos', 'Apple Calendar', 'Apple Contacts'];
    for (const label of labels) {
      expect(summary.textContent).toContain(label);
    }

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

    await waitFor(() => {
      expect(screen.getByTestId('apple-import-failures')).toBeTruthy();
    });
    expect(screen.queryByRole('button', { name: /^Continue$/ })).toBeNull();
    expect(onNext).not.toHaveBeenCalled();
  });
});

describe('AppleDataStep — Privacy & Automation deep link', () => {
  let onNext: () => void;
  let onSkip: () => void;
  let openSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    onNext = vi.fn();
    onSkip = vi.fn();
    openSpy = vi.spyOn(window, 'open').mockImplementation(() => null);
  });

  afterEach(() => {
    openSpy.mockRestore();
  });

  const renderStep = () =>
    renderWithRedux(<AppleDataStep onNext={onNext} onSkip={onSkip} />);

  it('renders the Privacy → Automation deep link before the user clicks Import', async () => {
    renderStep();
    await screen.findByRole('button', { name: /Import Selected/i });
    expect(screen.getByTestId('apple-permissions-deeplink')).toBeTruthy();
  });

  it('clicking the deep link launches the Privacy_Automation URL', async () => {
    renderStep();
    fireEvent.click(await screen.findByTestId('apple-permissions-deeplink'));
    expect(openSpy).toHaveBeenCalledWith(
      expect.stringContaining('x-apple.systempreferences:com.apple.preference.security?Privacy_Automation'),
      expect.anything(),
    );
  });

  it('renders the deep link inside the import-failures panel after a failed kickoff', async () => {
    renderStep();
    const btn = await screen.findByRole('button', { name: /Import Selected/i });
    fireEvent.click(btn);

    await screen.findByTestId('apple-import-failures');
    // After kickoff failure the deep link is still reachable so the user can
    // jump straight to System Settings if the error message hinted at TCC.
    expect(screen.getByTestId('apple-permissions-deeplink')).toBeTruthy();
  });

  it('renders the deep link when a per-source job fails (e.g. TCC denied mid-extract)', async () => {
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

    ingestionMock.getJobProgress.mockReset().mockImplementation((pid: string) =>
      Promise.resolve(
        pid === 'pid-contacts'
          ? {
              success: true,
              data: {
                progress_percentage: 0,
                status_message: 'Contacts access not granted (probe: ...). Grant access in System Settings → Privacy & Security → Automation, then retry.',
                error_message: 'Contacts access not granted. Grant access in System Settings → Privacy & Security → Automation, then retry.',
                is_complete: false,
                is_failed: true,
              },
            }
          : {
              success: true,
              data: {
                progress_percentage: 100,
                status_message: 'Done',
                is_complete: true,
                is_failed: false,
                results: { source: pid, total: 0, ingested: 0 },
              },
            },
      ),
    );

    renderStep();
    fireEvent.click(await screen.findByRole('button', { name: /Import Selected/i }));

    await waitFor(() => {
      expect(screen.getByTestId('apple-permissions-deeplink')).toBeTruthy();
    });
  });
});

describe('AppleDataStep — completed-import counts', () => {
  let onNext: () => void;
  let onSkip: () => void;

  beforeEach(() => {
    onNext = vi.fn();
    onSkip = vi.fn();
  });

  const renderStep = () =>
    renderWithRedux(<AppleDataStep onNext={onNext} onSkip={onSkip} />);

  it('renders the imported count from the structured results payload, not by parsing status_message', async () => {
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
});
