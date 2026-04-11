import { useState, useEffect, useRef, useCallback } from 'react';
import { BellIcon, XMarkIcon } from '@heroicons/react/24/outline';
import { discoveryClient } from '../api/clients/discoveryClient';

function formatRelativeTime(dateStr) {
  const now = new Date();
  const date = new Date(dateStr);
  const diffMs = now - date;
  const diffMins = Math.floor(diffMs / 60000);
  if (diffMins < 1) return 'just now';
  if (diffMins < 60) return `${diffMins}m ago`;
  const diffHrs = Math.floor(diffMins / 60);
  if (diffHrs < 24) return `${diffHrs}h ago`;
  const diffDays = Math.floor(diffHrs / 24);
  if (diffDays < 7) return `${diffDays}d ago`;
  return date.toLocaleDateString();
}

export default function NotificationsModal({ isOpen, onClose }) {
  const [notifications, setNotifications] = useState([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);
  const [dismissingIds, setDismissingIds] = useState(new Set());
  const modalRef = useRef(null);
  const previousFocusRef = useRef(null);

  // Load notifications when modal opens
  useEffect(() => {
    if (!isOpen) return;
    loadNotifications();
  }, [isOpen]);

  async function loadNotifications() {
    setLoading(true);
    setError(null);
    try {
      const res = await discoveryClient.listNotifications();
      if (res.success && res.data) {
        setNotifications(res.data.notifications || []);
      } else {
        setError(res.error || 'Failed to load notifications');
      }
    } catch (e) {
      setError(e.message || 'Failed to load notifications');
    }
    setLoading(false);
  }

  async function handleDismiss(id) {
    setDismissingIds(prev => new Set(prev).add(id));
    try {
      await discoveryClient.dismissNotification(id);
      setNotifications(prev => prev.filter(n => n.id !== id));
    } catch {
      // dismiss is best-effort
    } finally {
      setDismissingIds(prev => {
        const next = new Set(prev);
        next.delete(id);
        return next;
      });
    }
  }

  async function handleDismissAll() {
    const ids = notifications.map(n => n.id);
    setDismissingIds(new Set(ids));
    try {
      await Promise.all(ids.map(id => discoveryClient.dismissNotification(id)));
      setNotifications([]);
    } catch {
      // best-effort — reload to see what's left
      await loadNotifications();
    } finally {
      setDismissingIds(new Set());
    }
  }

  // Focus trap and restore
  useEffect(() => {
    if (!isOpen) return;
    previousFocusRef.current = document.activeElement;
    const modal = modalRef.current;
    if (modal) {
      const firstFocusable = modal.querySelector('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])');
      if (firstFocusable) firstFocusable.focus();
    }
    return () => {
      if (previousFocusRef.current && typeof previousFocusRef.current.focus === 'function') {
        previousFocusRef.current.focus();
      }
    };
  }, [isOpen]);

  const handleKeyDown = useCallback((e) => {
    if (e.key === 'Escape') { onClose(); return; }
    if (e.key !== 'Tab') return;
    const modal = modalRef.current;
    if (!modal) return;
    const focusable = modal.querySelectorAll('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])');
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault(); last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault(); first.focus();
    }
  }, [onClose]);

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-background/80 backdrop-blur-sm z-50 flex items-center justify-center p-4" onClick={onClose} onKeyDown={handleKeyDown}>
      <div ref={modalRef} role="dialog" aria-modal="true" aria-label="Notifications" className="bg-surface border border-border rounded-xl shadow-xl max-w-md w-full overflow-hidden flex flex-col max-h-[85vh]" onClick={(e) => e.stopPropagation()}>

        {/* Header */}
        <div className="px-6 py-4 border-b border-border flex items-center justify-between sticky top-0 bg-surface">
          <div className="flex items-center gap-3">
            <div className="p-2 bg-gruvbox-blue/10 rounded-lg text-gruvbox-blue">
              <BellIcon className="w-5 h-5" />
            </div>
            <div>
              <h2 className="text-lg font-medium text-primary">Notifications</h2>
              <p className="text-sm text-secondary">Shared data from your contacts</p>
            </div>
          </div>
          <div className="flex items-center gap-2">
            {notifications.length > 0 && (
              <button
                onClick={handleDismissAll}
                className="text-xs text-tertiary hover:text-primary transition-colors bg-transparent border-none cursor-pointer"
              >
                Dismiss All
              </button>
            )}
            <button
              onClick={onClose}
              className="p-2 text-tertiary hover:text-primary rounded-lg hover:bg-gruvbox-hover transition-colors"
              aria-label="Close notifications"
            >
              <XMarkIcon className="w-5 h-5" />
            </button>
          </div>
        </div>

        {/* List Body */}
        <div className="p-6 overflow-y-auto flex-1">
          {error && (
            <div className="mb-4 p-3 bg-gruvbox-red/10 border border-gruvbox-red/20 rounded-lg text-sm text-gruvbox-red">
              {error}
            </div>
          )}

          {loading ? (
            <div className="text-center py-8">
              <p className="text-secondary text-sm">Loading notifications...</p>
            </div>
          ) : notifications.length === 0 ? (
            <div className="text-center py-8">
              <div className="inline-flex justify-center items-center w-12 h-12 rounded-full bg-gruvbox-hover text-tertiary mb-3">
                <BellIcon className="w-6 h-6" />
              </div>
              <h3 className="text-primary font-medium">No notifications</h3>
              <p className="text-secondary text-sm mt-1">You&apos;re all caught up.</p>
            </div>
          ) : (
            <div className="space-y-3">
              {notifications.map((n) => {
                const isDismissing = dismissingIds.has(n.id);
                return (
                  <div key={n.id} className="p-4 rounded-xl border border-border bg-gruvbox-hover transition-all">
                    <div className="flex items-start justify-between gap-3">
                      <div className="flex-1 min-w-0">
                        <p className="text-sm text-primary">
                          <span className="font-medium">{n.sender_display_name}</span>
                          {' shared '}
                          <span className="font-medium">{n.records_received}</span>
                          {n.records_received === 1 ? ' record' : ' records'}
                        </p>
                        {n.schema_names && n.schema_names.length > 0 && (
                          <div className="flex flex-wrap gap-1 mt-2">
                            {n.schema_names.map((s) => (
                              <span key={s} className="px-1.5 py-0.5 text-xs rounded bg-gruvbox-blue/15 text-gruvbox-blue font-mono">
                                {s}
                              </span>
                            ))}
                          </div>
                        )}
                        <p className="text-xs text-tertiary mt-2">{formatRelativeTime(n.received_at)}</p>
                      </div>
                      <button
                        onClick={() => handleDismiss(n.id)}
                        disabled={isDismissing}
                        className="text-xs text-tertiary hover:text-primary transition-colors bg-transparent border-none cursor-pointer flex-shrink-0 px-2 py-1 rounded hover:bg-gruvbox-hover"
                      >
                        {isDismissing ? '...' : 'Dismiss'}
                      </button>
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
