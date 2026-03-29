import React from 'react';
import { screen, fireEvent, waitFor } from '@testing-library/react';
import { vi, describe, it, expect, beforeEach, afterEach } from 'vitest';
import OnboardingWizard, { ONBOARDING_STORAGE_KEY } from '../../../components/onboarding/OnboardingWizard';
import { renderWithRedux } from '../../utils/testHelpers.jsx';

// Mock child step components to isolate wizard logic
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

  it('renders the first step (AI Setup) by default', () => {
    renderWizard();
    expect(screen.getByTestId('ai-step')).toBeTruthy();
    expect(screen.getByText('FoldDB')).toBeTruthy();
  });

  it('shows progress indicator with 5 steps', () => {
    renderWizard();
    expect(screen.getByText('AI Setup')).toBeTruthy();
    expect(screen.getByText('Apple Data')).toBeTruthy();
    expect(screen.getByText('Cloud Backup')).toBeTruthy();
    expect(screen.getByText('Community')).toBeTruthy();
    expect(screen.getByText('All Set')).toBeTruthy();
  });

  it('navigates through all steps via Next buttons', () => {
    renderWizard();

    // Step 1: AI Setup -> Apple Data
    fireEvent.click(screen.getByTestId('ai-next'));
    expect(screen.getByTestId('apple-step')).toBeTruthy();

    // Step 2: Apple Data -> Cloud Backup
    fireEvent.click(screen.getByTestId('apple-next'));
    expect(screen.getByTestId('cloud-step')).toBeTruthy();

    // Step 3: Cloud Backup -> Discovery
    fireEvent.click(screen.getByTestId('cloud-next'));
    expect(screen.getByTestId('discovery-step')).toBeTruthy();

    // Step 4: Discovery -> All Set
    fireEvent.click(screen.getByTestId('discovery-next'));
    expect(screen.getByTestId('allset-step')).toBeTruthy();
  });

  it('allows skipping steps', () => {
    renderWizard();

    // Skip AI step
    fireEvent.click(screen.getByTestId('ai-skip'));
    expect(screen.getByTestId('apple-step')).toBeTruthy();

    // Skip Apple step
    fireEvent.click(screen.getByTestId('apple-skip'));
    expect(screen.getByTestId('cloud-step')).toBeTruthy();
  });

  it('tracks completed steps vs skipped', () => {
    renderWizard();

    // Complete AI step (Next)
    fireEvent.click(screen.getByTestId('ai-next'));
    // Skip Apple step
    fireEvent.click(screen.getByTestId('apple-skip'));
    // Complete Cloud step
    fireEvent.click(screen.getByTestId('cloud-next'));
    // Skip Discovery
    fireEvent.click(screen.getByTestId('discovery-skip'));

    // AllSet step should show 2 completed (welcome + cloud-backup)
    expect(screen.getByTestId('allset-step')).toBeTruthy();
    expect(screen.getByTestId('completed-count').textContent).toBe('2');
  });

  it('calls onComplete and saves to localStorage on finish', () => {
    renderWizard();

    // Navigate to All Set
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
});
