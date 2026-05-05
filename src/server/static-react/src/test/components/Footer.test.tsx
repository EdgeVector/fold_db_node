/**
 * @fileoverview Tests for Footer component
 *
 * Tests the Footer component rendering and content display. Version drift
 * guard: the footer must render whatever /api/health returns, never a
 * compile-time literal — see daemon.rs/system.rs for why
 * FOLDDB_BUILD_VERSION is the source of truth.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import Footer from '../../components/Footer';

const { HEALTH_VERSION } = vi.hoisted(() => ({
  HEALTH_VERSION: '9.9.9-from-health',
}));

vi.mock('../../api/clients/systemClient', () => ({
  systemClient: {
    getDatabaseConfig: vi.fn().mockResolvedValue({ data: { type: 'local' } }),
    getHealth: vi.fn().mockResolvedValue({
      data: { ok: true, version: HEALTH_VERSION, uptime_s: 42 },
    }),
  },
}));

beforeEach(() => {
  localStorage.clear();
});

describe('Footer Component', () => {
  it('renders footer with structure', () => {
    render(<Footer />);

    const footer = screen.getByRole('contentinfo');
    expect(footer).toBeInTheDocument();
    expect(footer).toHaveClass('bg-surface', 'border-t', 'flex-shrink-0');
  });

  it('displays FoldDB branding', () => {
    render(<Footer />);

    expect(screen.getByText(/FoldDB/i)).toBeInTheDocument();
  });

  it('displays the version returned by /api/health', async () => {
    render(<Footer />);

    await waitFor(() => {
      expect(screen.getByText(`FoldDB v${HEALTH_VERSION}`)).toBeInTheDocument();
    });
  });

  it('displays Local Mode indicator by default', () => {
    render(<Footer />);

    expect(screen.getByText('Local Mode')).toBeInTheDocument();
  });

  it('displays Cloud Mode when exemem credentials exist in localStorage', async () => {
    localStorage.setItem('exemem_api_url', 'https://example.com');
    localStorage.setItem('exemem_api_key', 'test-key');

    render(<Footer />);

    await waitFor(() => {
      expect(screen.getByText('Cloud Mode')).toBeInTheDocument();
    });
  });

  it('has proper layout structure', () => {
    render(<Footer />);

    const footer = screen.getByRole('contentinfo');
    const container = footer.firstChild;
    expect(container).toHaveClass('flex', 'items-center', 'justify-between');
  });

  it('renders without crashing', () => {
    expect(() => render(<Footer />)).not.toThrow();
  });
});
