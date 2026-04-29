/**
 * @fileoverview Tests for the shared ErrorMessage component.
 *
 * The whole point of this primitive is that Rust-shaped strings
 * (function names, snake_case identifiers, "Internal error: ...")
 * never appear as the headline a user reads. They get tucked behind
 * a "Technical details" disclosure with a friendly fallback.
 *
 * If you change `looksTechnical`, update these cases — the heuristic
 * is the contract.
 */

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { ErrorMessage, looksTechnical } from '../../../components/shared/ErrorMessage.jsx';

describe('looksTechnical', () => {
  it('flags the canonical leaked Rust error from Personas', () => {
    const msg =
      "Internal error: fingerprints: canonical_names not initialized for 'Persona': Configuration error: canonical_names: registry not initialized — register_phase_1_schemas() must run at subsystem startup before any lookup. Attempted lookup: 'Persona'";
    expect(looksTechnical(msg)).toBe(true);
  });

  it('flags any "Internal error:" prefix', () => {
    expect(looksTechnical('Internal error: something blew up')).toBe(true);
  });

  it('flags Rust path syntax', () => {
    expect(looksTechnical('foo::bar exploded')).toBe(true);
  });

  it('flags function call references', () => {
    expect(looksTechnical('register_phase_1_schemas() must run at startup')).toBe(true);
  });

  it('flags strings with several snake_case identifiers', () => {
    expect(looksTechnical('canonical_names registry_init schema_registry failed')).toBe(true);
  });

  it('flags very long single messages', () => {
    expect(looksTechnical('x'.repeat(200))).toBe(true);
  });

  it('does NOT flag short friendly messages', () => {
    expect(looksTechnical('Network error')).toBe(false);
    expect(looksTechnical('Failed to load personas')).toBe(false);
    expect(looksTechnical("Couldn't reach the server")).toBe(false);
  });

  it('returns false for empty / null input', () => {
    expect(looksTechnical('')).toBe(false);
    expect(looksTechnical(null)).toBe(false);
    expect(looksTechnical(undefined)).toBe(false);
  });
});

describe('<ErrorMessage>', () => {
  it('renders nothing when error is null', () => {
    const { container } = render(<ErrorMessage error={null} fallback="x" />);
    expect(container.firstChild).toBeNull();
  });

  it('renders friendly fallback as headline when error is technical', () => {
    render(
      <ErrorMessage
        error="Internal error: register_phase_1_schemas() must run at subsystem startup"
        fallback="Couldn't load personas"
      />
    );
    expect(screen.getByText("Couldn't load personas")).toBeInTheDocument();
    // Raw text exists in the DOM (inside <details>) but the headline
    // is the friendly fallback. Open the disclosure to confirm.
    expect(screen.getByText('Technical details')).toBeInTheDocument();
  });

  it('renders the raw message inline when it is friendly', () => {
    render(<ErrorMessage error="Network error" fallback="Couldn't load personas" />);
    expect(screen.getByText('Network error')).toBeInTheDocument();
    // No technical-details disclosure when message is already friendly.
    expect(screen.queryByText('Technical details')).toBeNull();
  });

  it('shows the technical details when expanded', () => {
    const raw = 'Internal error: foo::bar() exploded with snake_case_one snake_case_two';
    render(<ErrorMessage error={raw} fallback="Boom" />);
    const summary = screen.getByText('Technical details');
    fireEvent.click(summary);
    // Use a function matcher because <pre> may break the text across nodes
    expect(
      screen.getByText((_, node) => node?.tagName === 'PRE' && node.textContent.includes(raw))
    ).toBeInTheDocument();
  });

  it('renders a Retry button when onRetry is provided', () => {
    const onRetry = vi.fn();
    render(<ErrorMessage error="Network error" fallback="x" onRetry={onRetry} />);
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(onRetry).toHaveBeenCalledOnce();
  });

  it('omits the Retry button when onRetry is not provided', () => {
    render(<ErrorMessage error="Network error" fallback="x" />);
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
  });

  it('respects a custom testId', () => {
    render(<ErrorMessage error="Network error" fallback="x" testId="my-error" />);
    expect(screen.getByTestId('my-error')).toBeInTheDocument();
  });
});
