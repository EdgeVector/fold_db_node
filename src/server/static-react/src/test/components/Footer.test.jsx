/**
 * @fileoverview Tests for Footer component
 *
 * Tests the Footer component rendering and content display.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import Footer from '../../components/Footer.jsx';

vi.mock('../../api/clients/systemClient', () => ({
  systemClient: {
    getDatabaseConfig: vi.fn().mockResolvedValue({ data: { type: 'local' } }),
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

  it('displays version number', () => {
    render(<Footer />);

    expect(screen.getByText(/v\d+\.\d+\.\d+/i)).toBeInTheDocument();
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
