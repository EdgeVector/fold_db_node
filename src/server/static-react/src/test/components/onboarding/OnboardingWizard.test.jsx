import React from 'react';
import { screen, fireEvent } from '@testing-library/react';
import { vi, describe, it, expect, beforeEach, afterEach } from 'vitest';
import OnboardingWizard, { ONBOARDING_STORAGE_KEY } from '../../../components/onboarding/OnboardingWizard';
import { renderWithRedux } from '../../utils/testUtilities.jsx';

// Mock child step components to isolate wizard logic
vi.mock('../../../components/onboarding/IdentityStep', () => ({
  default: ({ onNext, onSkip }) => (
    <div data-testid="identity-step">
      Identity Step
      <button data-testid="identity-next" onClick={onNext}>Next</button>
      <button data-testid="identity-skip" onClick={onSkip}>Skip</button>
    </div>
  ),
}));

vi.mock('../../../components/onboarding/ConfigureAiStep', () => ({
  default: ({ onNext, onSkip }) => (
    <div data-testid="ai-step">
      AI Setup Step
      <button data-testid="ai-next" onClick={onNext}>Next</button>
      <button data-testid="ai-skip" onClick={onSkip}>Skip</button>
    </div>
  ),
}));

vi.mock('../../../components/onboarding/AppleDataStep', () => ({
  default: ({ onNext, onSkip }) => (
    <div data-testid="apple-step">
      Apple Data Step
      <button data-testid="apple-next" onClick={onNext}>Next</button>
      <button data-testid="apple-skip" onClick={onSkip}>Skip</button>
    </div>
  ),
}));

vi.mock('../../../components/onboarding/CloudBackupStep', () => ({
  default: ({ onNext, onSkip }) => (
    <div data-testid="cloud-step">
      Cloud Backup Step
      <button data-testid="cloud-next" onClick={onNext}>Next</button>
      <button data-testid="cloud-skip" onClick={onSkip}>Skip</button>
    </div>
  ),
}));

vi.mock('../../../components/onboarding/DiscoveryStep', () => ({
  default: ({ onNext, onSkip }) => (
    <div data-testid="discovery-step">
      Discovery Step
      <button data-testid="discovery-next" onClick={onNext}>Next</button>
      <button data-testid="discovery-skip" onClick={onSkip}>Skip</button>
    </div>
  ),
}));

vi.mock('../../../components/onboarding/AllSetStep', () => ({
  default: ({ onFinish, completedSteps }) => (
    <div data-testid="allset-step">
      All Set Step
      <span data-testid="completed-count">{completedSteps?.size || 0}</span>
      <button data-testid="allset-finish" onClick={onFinish}>Finish</button>
    </div>
  ),
}));

describe('OnboardingWizard', () => {
  let onComplete;

  beforeEach(() => {
    onComplete = vi.fn();
    localStorage.clear();
  });

  afterEach(() => {
    localStorage.clear();
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

  it('navigates through all steps via Next buttons', () => {
    renderWizard();

    // Step 1: Identity -> AI Setup
    fireEvent.click(screen.getByTestId('identity-next'));
    expect(screen.getByTestId('ai-step')).toBeTruthy();

    // Step 2: AI Setup -> Apple Data
    fireEvent.click(screen.getByTestId('ai-next'));
    expect(screen.getByTestId('apple-step')).toBeTruthy();

    // Step 3: Apple Data -> Cloud Backup
    fireEvent.click(screen.getByTestId('apple-next'));
    expect(screen.getByTestId('cloud-step')).toBeTruthy();

    // Step 4: Cloud Backup -> Discovery
    fireEvent.click(screen.getByTestId('cloud-next'));
    expect(screen.getByTestId('discovery-step')).toBeTruthy();

    // Step 5: Discovery -> All Set
    fireEvent.click(screen.getByTestId('discovery-next'));
    expect(screen.getByTestId('allset-step')).toBeTruthy();
  });

  it('allows skipping steps', () => {
    renderWizard();

    // Skip Identity step
    fireEvent.click(screen.getByTestId('identity-skip'));
    expect(screen.getByTestId('ai-step')).toBeTruthy();

    // Skip AI step
    fireEvent.click(screen.getByTestId('ai-skip'));
    expect(screen.getByTestId('apple-step')).toBeTruthy();

    // Skip Apple step
    fireEvent.click(screen.getByTestId('apple-skip'));
    expect(screen.getByTestId('cloud-step')).toBeTruthy();
  });

  it('tracks completed steps vs skipped', () => {
    renderWizard();

    // Complete Identity step
    fireEvent.click(screen.getByTestId('identity-next'));
    // Complete AI step (Next)
    fireEvent.click(screen.getByTestId('ai-next'));
    // Skip Apple step
    fireEvent.click(screen.getByTestId('apple-skip'));
    // Complete Cloud step
    fireEvent.click(screen.getByTestId('cloud-next'));
    // Skip Discovery
    fireEvent.click(screen.getByTestId('discovery-skip'));

    // AllSet step should show 3 completed (identity + welcome + cloud-backup)
    expect(screen.getByTestId('allset-step')).toBeTruthy();
    expect(screen.getByTestId('completed-count').textContent).toBe('3');
  });

  it('calls onComplete and saves to localStorage on finish', () => {
    renderWizard();

    // Navigate to All Set by skipping everything
    fireEvent.click(screen.getByTestId('identity-skip'));
    fireEvent.click(screen.getByTestId('ai-skip'));
    fireEvent.click(screen.getByTestId('apple-skip'));
    fireEvent.click(screen.getByTestId('cloud-skip'));
    fireEvent.click(screen.getByTestId('discovery-skip'));

    // Click finish
    fireEvent.click(screen.getByTestId('allset-finish'));
    expect(onComplete).toHaveBeenCalledOnce();
    expect(localStorage.getItem(ONBOARDING_STORAGE_KEY)).toBe('1');
  });

  it('exports ONBOARDING_STORAGE_KEY constant', () => {
    expect(ONBOARDING_STORAGE_KEY).toBe('folddb_onboarding_complete');
  });

  describe('cloud-already-active gate', () => {
    it('skips CloudBackupStep and goes directly to Discovery when exemem_api_key is set', () => {
      localStorage.setItem('exemem_api_key', 'test-api-key-abc123');
      renderWizard();

      // Cloud Backup should not appear in the progress indicator.
      expect(screen.queryByText('Cloud Backup')).toBeNull();

      // Navigate: identity -> welcome -> apple-data -> (skip cloud) -> discovery.
      fireEvent.click(screen.getByTestId('identity-next'));
      fireEvent.click(screen.getByTestId('ai-next'));
      fireEvent.click(screen.getByTestId('apple-next'));

      // We should land on Discovery, NOT Cloud Backup.
      expect(screen.queryByTestId('cloud-step')).toBeNull();
      expect(screen.getByTestId('discovery-step')).toBeTruthy();
    });

    it('also skips cloud step when apple-data is skipped', () => {
      localStorage.setItem('exemem_api_key', 'test-api-key-abc123');
      renderWizard();

      fireEvent.click(screen.getByTestId('identity-skip'));
      fireEvent.click(screen.getByTestId('ai-skip'));
      fireEvent.click(screen.getByTestId('apple-skip'));

      expect(screen.queryByTestId('cloud-step')).toBeNull();
      expect(screen.getByTestId('discovery-step')).toBeTruthy();
    });

    it('marks cloud-backup completed when skipped via gate', () => {
      localStorage.setItem('exemem_api_key', 'test-api-key-abc123');
      renderWizard();

      fireEvent.click(screen.getByTestId('identity-next'));
      fireEvent.click(screen.getByTestId('ai-next'));
      fireEvent.click(screen.getByTestId('apple-next'));
      fireEvent.click(screen.getByTestId('discovery-next'));

      // identity + welcome + apple-data + cloud-backup (gated) + discovery = 5
      expect(screen.getByTestId('allset-step')).toBeTruthy();
      expect(screen.getByTestId('completed-count').textContent).toBe('5');
    });

    it('renders CloudBackupStep normally when exemem_api_key is not set', () => {
      // localStorage already cleared in beforeEach.
      expect(localStorage.getItem('exemem_api_key')).toBeNull();
      renderWizard();

      // Cloud Backup should appear in the progress indicator.
      expect(screen.getByText('Cloud Backup')).toBeTruthy();

      fireEvent.click(screen.getByTestId('identity-next'));
      fireEvent.click(screen.getByTestId('ai-next'));
      fireEvent.click(screen.getByTestId('apple-next'));

      // We should land on Cloud Backup, not Discovery.
      expect(screen.getByTestId('cloud-step')).toBeTruthy();
      expect(screen.queryByTestId('discovery-step')).toBeNull();
    });
  });
});
