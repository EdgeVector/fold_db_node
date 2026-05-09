import React from 'react';
import { screen, fireEvent, waitFor } from '@testing-library/react';
import { vi, describe, it, expect, beforeEach, afterEach } from 'vitest';
import OnboardingWizard, { ONBOARDING_STORAGE_KEY } from '../../../components/onboarding/OnboardingWizard';
import { renderWithRedux } from '../../utils/testUtilities';
import { systemClient } from '../../../api/clients/systemClient';

type StepMockProps = { onNext: () => void; onSkip: () => void };
type AllSetMockProps = { onFinish: () => void; completedSteps?: { size?: number } };
type CloudStepMockProps = {
  onEnableCloud: () => void;
  onRestore: () => void;
  onSkip: () => void;
  submitting: boolean;
  error: string | null;
};
type RecoveryViewMockProps = { words: string[]; onContinue: () => void };

// Mock child step components to isolate wizard logic. The wizard now passes
// controlled-input props (fields/onChange); the mocks ignore those and just
// expose Next/Skip buttons. CloudBackupStep is special — it triggers the
// bootstrap submission via three distinct callbacks.
vi.mock('../../../components/onboarding/IdentityStep', () => ({
  default: ({ onNext, onSkip }: StepMockProps) => (
    <div data-testid="identity-step">
      Identity Step
      <button data-testid="identity-next" onClick={onNext}>Next</button>
      <button data-testid="identity-skip" onClick={onSkip}>Skip</button>
    </div>
  ),
}));

vi.mock('../../../components/onboarding/ConfigureAiStep', () => ({
  default: ({ onNext, onSkip }: StepMockProps) => (
    <div data-testid="ai-step">
      AI Setup Step
      <button data-testid="ai-next" onClick={onNext}>Next</button>
      <button data-testid="ai-skip" onClick={onSkip}>Skip</button>
    </div>
  ),
}));

vi.mock('../../../components/onboarding/AppleDataStep', () => ({
  default: ({ onNext, onSkip }: StepMockProps) => (
    <div data-testid="apple-step">
      Apple Data Step
      <button data-testid="apple-next" onClick={onNext}>Next</button>
      <button data-testid="apple-skip" onClick={onSkip}>Skip</button>
    </div>
  ),
}));

vi.mock('../../../components/onboarding/CloudBackupStep', () => ({
  default: ({ onEnableCloud, onRestore, onSkip, submitting, error }: CloudStepMockProps) => (
    <div data-testid="cloud-step">
      Cloud Backup Step
      <span data-testid="cloud-submitting">{submitting ? 'yes' : 'no'}</span>
      {error ? <span data-testid="cloud-error">{error}</span> : null}
      <button data-testid="cloud-enable" onClick={onEnableCloud}>Enable</button>
      <button data-testid="cloud-restore" onClick={onRestore}>Restore</button>
      <button data-testid="cloud-skip" onClick={onSkip}>Skip</button>
    </div>
  ),
}));

vi.mock('../../../components/onboarding/RecoveryPhraseView', () => ({
  default: ({ words, onContinue }: RecoveryViewMockProps) => (
    <div data-testid="recovery-step">
      <span data-testid="recovery-word-count">{words.length}</span>
      <button data-testid="recovery-continue" onClick={onContinue}>Continue</button>
    </div>
  ),
}));

vi.mock('../../../components/onboarding/DiscoveryStep', () => ({
  default: ({ onNext, onSkip }: StepMockProps) => (
    <div data-testid="discovery-step">
      Discovery Step
      <button data-testid="discovery-next" onClick={onNext}>Next</button>
      <button data-testid="discovery-skip" onClick={onSkip}>Skip</button>
    </div>
  ),
}));

vi.mock('../../../components/onboarding/AllSetStep', () => ({
  default: ({ onFinish, completedSteps }: AllSetMockProps) => (
    <div data-testid="allset-step">
      All Set Step
      <span data-testid="completed-count">{completedSteps?.size || 0}</span>
      <button data-testid="allset-finish" onClick={onFinish}>Finish</button>
    </div>
  ),
}));

const FRESH_BOOTSTRAP = {
  success: true,
  status: 200,
  data: {
    public_key: 'pk',
    user_hash: 'uh',
    recovery_phrase: Array.from({ length: 24 }, (_, i) => `word${i + 1}`),
  },
};

const RESTORE_BOOTSTRAP = {
  success: true,
  status: 200,
  data: { public_key: 'pk', user_hash: 'uh' },
};

describe('OnboardingWizard', () => {
  let onComplete: () => void;
  let bootstrapSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    onComplete = vi.fn();
    localStorage.clear();
    bootstrapSpy = vi
      .spyOn(systemClient, 'bootstrap')
      .mockResolvedValue(FRESH_BOOTSTRAP as never);
  });

  afterEach(() => {
    localStorage.clear();
    vi.restoreAllMocks();
  });

  const renderWizard = () => {
    return renderWithRedux(<OnboardingWizard onComplete={onComplete} />);
  };

  it('renders the first step (Identity) by default', () => {
    renderWizard();
    expect(screen.getByTestId('identity-step')).toBeTruthy();
    expect(screen.getByText('FoldDB')).toBeTruthy();
  });

  it('shows progress indicator with 6 steps', () => {
    renderWizard();
    expect(screen.getByText('Identity')).toBeTruthy();
    expect(screen.getByText('AI Setup')).toBeTruthy();
    expect(screen.getByText('Apple Data')).toBeTruthy();
    expect(screen.getByText('Cloud Backup')).toBeTruthy();
    expect(screen.getByText('Community')).toBeTruthy();
    expect(screen.getByText('All Set')).toBeTruthy();
  });

  it('collapses onboarding to a single bootstrap POST and renders the recovery phrase', async () => {
    renderWizard();

    fireEvent.click(screen.getByTestId('identity-next'));
    expect(screen.getByTestId('ai-step')).toBeTruthy();

    fireEvent.click(screen.getByTestId('ai-next'));
    expect(screen.getByTestId('cloud-step')).toBeTruthy();

    fireEvent.click(screen.getByTestId('cloud-enable'));

    await waitFor(() => {
      expect(bootstrapSpy).toHaveBeenCalledTimes(1);
    });
    expect(bootstrapSpy.mock.calls[0][0]).toMatchObject({
      enable_cloud: true,
      recovery_phrase: null,
    });

    expect(screen.getByTestId('recovery-step')).toBeTruthy();
    expect(screen.getByTestId('recovery-word-count').textContent).toBe('24');

    fireEvent.click(screen.getByTestId('recovery-continue'));
    expect(screen.getByTestId('apple-step')).toBeTruthy();

    fireEvent.click(screen.getByTestId('apple-next'));
    expect(screen.getByTestId('discovery-step')).toBeTruthy();

    fireEvent.click(screen.getByTestId('discovery-next'));
    expect(screen.getByTestId('allset-step')).toBeTruthy();
  });

  it('skip on cloud step still POSTs once with enable_cloud=false', async () => {
    renderWizard();

    fireEvent.click(screen.getByTestId('identity-skip'));
    fireEvent.click(screen.getByTestId('ai-skip'));
    fireEvent.click(screen.getByTestId('cloud-skip'));

    await waitFor(() => {
      expect(bootstrapSpy).toHaveBeenCalledTimes(1);
    });
    expect(bootstrapSpy.mock.calls[0][0]).toMatchObject({
      enable_cloud: false,
      invite_code: null,
      recovery_phrase: null,
    });

    expect(screen.getByTestId('recovery-step')).toBeTruthy();
  });

  it('restore path skips the recovery phrase view (no fresh-mint phrase in response)', async () => {
    bootstrapSpy.mockResolvedValueOnce(RESTORE_BOOTSTRAP as never);
    renderWizard();

    fireEvent.click(screen.getByTestId('identity-next'));
    fireEvent.click(screen.getByTestId('ai-next'));
    fireEvent.click(screen.getByTestId('cloud-restore'));

    await waitFor(() => {
      expect(bootstrapSpy).toHaveBeenCalledTimes(1);
    });
    expect(bootstrapSpy.mock.calls[0][0]).toMatchObject({
      enable_cloud: true,
    });
    expect(screen.queryByTestId('recovery-step')).toBeNull();
    expect(screen.getByTestId('apple-step')).toBeTruthy();
  });

  it('surfaces bootstrap failure inline without advancing', async () => {
    bootstrapSpy.mockRejectedValueOnce(new Error('boom'));
    renderWizard();

    fireEvent.click(screen.getByTestId('identity-next'));
    fireEvent.click(screen.getByTestId('ai-next'));
    fireEvent.click(screen.getByTestId('cloud-enable'));

    await waitFor(() => {
      expect(screen.getByTestId('cloud-error')).toBeTruthy();
    });
    expect(screen.queryByTestId('recovery-step')).toBeNull();
    expect(screen.queryByTestId('apple-step')).toBeNull();
  });

  it('writes localStorage marker as soon as bootstrap succeeds (before user reaches All Set)', async () => {
    renderWizard();

    fireEvent.click(screen.getByTestId('identity-next'));
    fireEvent.click(screen.getByTestId('ai-next'));
    fireEvent.click(screen.getByTestId('cloud-skip'));

    await waitFor(() => {
      expect(bootstrapSpy).toHaveBeenCalledTimes(1);
    });
    // Marker is set immediately on bootstrap success — the user does not
    // need to walk all the way to AllSetStep + click Finish for a reload
    // to skip the wizard.
    await waitFor(() => {
      expect(localStorage.getItem(ONBOARDING_STORAGE_KEY)).toBe('1');
    });
    // Still on the recovery view — onComplete has NOT been called.
    expect(screen.getByTestId('recovery-step')).toBeTruthy();
    expect(onComplete).not.toHaveBeenCalled();
  });

  it('does not write localStorage marker if bootstrap fails', async () => {
    bootstrapSpy.mockRejectedValueOnce(new Error('boom'));
    renderWizard();

    fireEvent.click(screen.getByTestId('identity-next'));
    fireEvent.click(screen.getByTestId('ai-next'));
    fireEvent.click(screen.getByTestId('cloud-enable'));

    await waitFor(() => {
      expect(screen.getByTestId('cloud-error')).toBeTruthy();
    });
    expect(localStorage.getItem(ONBOARDING_STORAGE_KEY)).toBeNull();
  });

  it('calls onComplete and writes localStorage on finish — bootstrap writes the marker, no extra call', () => {
    renderWizard();

    // Use the global "Skip setup entirely" link to bail without finishing.
    // The wizard never calls /system/onboarding-complete; the bootstrap
    // marker is the source of truth.
    fireEvent.click(screen.getByText('Skip setup entirely'));
    expect(onComplete).toHaveBeenCalledOnce();
    expect(localStorage.getItem(ONBOARDING_STORAGE_KEY)).toBe('1');
    expect(bootstrapSpy).not.toHaveBeenCalled();
  });

  it('exports ONBOARDING_STORAGE_KEY constant', () => {
    expect(ONBOARDING_STORAGE_KEY).toBe('folddb_onboarding_complete');
  });

  describe('cloud-already-active gate', () => {
    it('skips CloudBackupStep and goes directly to Apple when exemem_api_key is set', () => {
      localStorage.setItem('exemem_api_key', 'test-api-key-abc123');
      renderWizard();

      // Cloud Backup should not appear in the progress indicator.
      expect(screen.queryByText('Cloud Backup')).toBeNull();

      // Navigate: identity -> ai -> apple-data (cloud step bypassed).
      fireEvent.click(screen.getByTestId('identity-next'));
      fireEvent.click(screen.getByTestId('ai-next'));

      expect(screen.queryByTestId('cloud-step')).toBeNull();
      expect(screen.getByTestId('apple-step')).toBeTruthy();
    });

    it('renders CloudBackupStep normally when exemem_api_key is not set', () => {
      // localStorage already cleared in beforeEach.
      expect(localStorage.getItem('exemem_api_key')).toBeNull();
      renderWizard();

      // Cloud Backup should appear in the progress indicator.
      expect(screen.getByText('Cloud Backup')).toBeTruthy();

      fireEvent.click(screen.getByTestId('identity-next'));
      fireEvent.click(screen.getByTestId('ai-next'));

      // We should land on Cloud Backup, not Apple Data.
      expect(screen.getByTestId('cloud-step')).toBeTruthy();
      expect(screen.queryByTestId('apple-step')).toBeNull();
    });
  });
});
