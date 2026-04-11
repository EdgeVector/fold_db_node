import { useState, useEffect, useRef, useCallback } from 'react';
import { XMarkIcon } from '@heroicons/react/24/outline';
import { discoveryClient } from '../api/clients/discoveryClient';
import { listContacts } from '../api/clients/trustClient';

export default function ShareRecordModal({ schemaName, recordKey, isOpen, onClose }) {
  const [contacts, setContacts] = useState([]);
  const [loading, setLoading] = useState(true);
  const [selectedContact, setSelectedContact] = useState(null);
  const [sharing, setSharing] = useState(false);
  const [result, setResult] = useState(null);
  const [error, setError] = useState(null);
  const modalRef = useRef(null);
  const previousFocusRef = useRef(null);

  useEffect(() => {
    if (isOpen) {
      loadContacts();
      setResult(null);
      setError(null);
      setSelectedContact(null);
    }
  }, [isOpen]);

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

  async function loadContacts() {
    setLoading(true);
    try {
      const resp = await listContacts();
      if (resp.success && resp.data) {
        setContacts((resp.data.contacts || []).filter(c => !c.revoked));
      }
    } catch {
      setError('Failed to load contacts');
    }
    setLoading(false);
  }

  async function handleShare() {
    if (!selectedContact) return;
    setSharing(true);
    setError(null);
    try {
      const keyStr = recordKey.range || recordKey.hash;
      const resp = await discoveryClient.shareData(
        selectedContact.public_key,
        [{ schema_name: schemaName, record_key: keyStr }]
      );
      if (resp.success) {
        setResult(`Shared with ${selectedContact.display_name}`);
      } else {
        setError(resp.error || 'Share failed');
      }
    } catch (e) {
      setError(e.message || 'Share failed');
    }
    setSharing(false);
  }

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-background/80 backdrop-blur-sm z-50 flex items-center justify-center p-4" onClick={onClose} onKeyDown={handleKeyDown}>
      <div ref={modalRef} role="dialog" aria-modal="true" aria-label="Share Record" className="bg-surface border border-border rounded-xl shadow-xl max-w-md w-full overflow-hidden flex flex-col max-h-[85vh]" onClick={(e) => e.stopPropagation()}>

        {/* Header */}
        <div className="px-6 py-4 border-b border-border flex items-center justify-between sticky top-0 bg-surface">
          <h3 className="text-base font-medium text-primary">Share Record</h3>
          <button
            onClick={onClose}
            className="p-2 text-tertiary hover:text-primary rounded-lg hover:bg-gruvbox-hover transition-colors"
            aria-label="Close share dialog"
          >
            <XMarkIcon className="w-5 h-5" />
          </button>
        </div>

        {/* Body */}
        <div className="p-6 overflow-y-auto flex-1">
          <p className="text-sm text-secondary mb-4">
            Sharing <span className="font-mono text-primary">{schemaName}</span>
          </p>

          {loading && <p className="text-sm text-secondary">Loading contacts...</p>}

          {!loading && contacts.length === 0 && (
            <p className="text-sm text-secondary">No contacts. Connect with someone via Discovery first.</p>
          )}

          {!loading && contacts.length > 0 && (
            <div className="space-y-2">
              {contacts.map(c => (
                <button
                  key={c.public_key}
                  type="button"
                  className={`w-full text-left p-3 rounded-lg border transition-colors ${
                    selectedContact?.public_key === c.public_key
                      ? 'border-gruvbox-blue bg-gruvbox-blue/10'
                      : 'border-border hover:border-gruvbox-blue/50'
                  }`}
                  onClick={() => setSelectedContact(c)}
                >
                  <span className="text-sm font-medium text-primary">{c.display_name}</span>
                  {c.contact_hint && <span className="text-xs text-secondary ml-2">{c.contact_hint}</span>}
                </button>
              ))}
            </div>
          )}

          {error && <p className="text-sm text-gruvbox-red mt-3">{error}</p>}
          {result && <p className="text-sm text-gruvbox-green mt-3">{result}</p>}
        </div>

        {/* Footer */}
        <div className="px-6 py-4 border-t border-border flex justify-end gap-3">
          <button onClick={onClose} className="btn btn-sm">Cancel</button>
          <button
            onClick={handleShare}
            disabled={!selectedContact || sharing || !!result}
            className="btn btn-sm btn-primary"
          >
            {sharing ? 'Sharing...' : result ? 'Done' : 'Share'}
          </button>
        </div>
      </div>
    </div>
  );
}
