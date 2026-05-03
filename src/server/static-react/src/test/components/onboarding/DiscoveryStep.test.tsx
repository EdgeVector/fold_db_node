import React from 'react';
import { screen, fireEvent, waitFor } from '@testing-library/react';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import DiscoveryStep from '../../../components/onboarding/DiscoveryStep';
import { renderWithRedux } from '../../utils/testUtilities';

vi.mock('../../../hooks/useApprovedSchemas', () => ({
  useApprovedSchemas: () => ({ approvedSchemas: [] }),
}));

vi.mock('../../../api/clients/discoveryClient', () => ({
  discoveryClient: {
    listOptIns: vi.fn().mockResolvedValue({ success: true }),
    optIn: vi.fn().mockResolvedValue({ success: true }),
  },
}));

describe('DiscoveryStep — alpha gap 4ae28 (Community CTA always visible)', () => {
  let onNext: () => void;
  let onSkip: () => void;

  beforeEach(() => {
    onNext = vi.fn();
    onSkip = vi.fn();
  });

  const renderStep = () =>
    renderWithRedux(<DiscoveryStep onNext={onNext} onSkip={onSkip} />);

  const findCta = async () =>
    (await screen.findByRole('button', { name: /Join & Continue/i })) as HTMLButtonElement;

  it('renders the primary "Join & Continue" CTA before any interest is selected', async () => {
    renderStep();
    const cta = await findCta();
    expect(cta).toBeTruthy();
    expect(cta.disabled).toBe(true);
    expect(cta.getAttribute('aria-disabled')).toBe('true');
    expect(cta.getAttribute('title')).toMatch(/select at least one interest/i);
  });

  it('still shows a Skip button alongside the disabled CTA', async () => {
    renderStep();
    await findCta();
    expect(screen.getByRole('button', { name: /^Skip$/ })).toBeTruthy();
  });

  it('enables the CTA once an interest is toggled on', async () => {
    renderStep();
    const cta = await findCta();
    expect(cta.disabled).toBe(true);

    fireEvent.click(screen.getByRole('button', { name: /Personal Notes/i }));

    await waitFor(() => {
      expect(cta.disabled).toBe(false);
      expect(cta.getAttribute('aria-disabled')).toBe('false');
      expect(cta.getAttribute('title')).toBeNull();
    });
  });

  it('re-disables the CTA if the last selected interest is toggled off', async () => {
    renderStep();
    const cta = await findCta();
    const tag = screen.getByRole('button', { name: /Personal Notes/i });

    fireEvent.click(tag);
    await waitFor(() => expect(cta.disabled).toBe(false));

    fireEvent.click(tag);
    await waitFor(() => expect(cta.disabled).toBe(true));
  });
});
