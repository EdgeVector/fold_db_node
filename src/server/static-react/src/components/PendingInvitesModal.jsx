import { useState } from 'react';
import { EnvelopeIcon, CheckIcon, XMarkIcon, BuildingOfficeIcon } from '@heroicons/react/24/outline';
import { orgClient } from '../api/clients/orgClient';

export default function PendingInvitesModal({ isOpen, onClose, pendingInvites, setPendingInvites }) {
  const [loadingIds, setLoadingIds] = useState(new Set());
  const [error, setError] = useState(null);

  if (!isOpen) return null;

  // Use org_public_key as the unique identifier since org_hash is not in the invite bundle
  const inviteId = (invite) => invite.org_public_key || invite.org_name;

  const handleAccept = async (invite) => {
    const id = inviteId(invite);
    try {
      setLoadingIds(prev => new Set(prev).add(id));
      setError(null);
      const res = await orgClient.joinOrg(invite);
      if (res.data) {
        setPendingInvites(prev => prev.filter(inv => inviteId(inv) !== id));
      }
    } catch (err) {
      console.error('Failed to accept invite', err);
      setError(err.message || 'Failed to join organization. Try again.');
    } finally {
      setLoadingIds(prev => {
        const next = new Set(prev);
        next.delete(id);
        return next;
      });
    }
  };

  const handleDecline = async (invite) => {
    const id = inviteId(invite);
    try {
      setLoadingIds(prev => new Set(prev).add(id));
      setError(null);
      // declineInvite needs org_hash, but the invite bundle only has org_public_key.
      // The backend join response returns org_hash, but for decline we need to derive it.
      // For now, pass the org_public_key — the backend will need to handle this.
      // TODO: add org_hash to the invite bundle on the backend
      await orgClient.declineInvite(invite.org_hash || invite.org_public_key);
      setPendingInvites(prev => prev.filter(inv => inviteId(inv) !== id));
    } catch (err) {
      console.error('Failed to decline invite', err);
      setError(err.message || 'Failed to decline. Try again.');
    } finally {
      setLoadingIds(prev => {
        const next = new Set(prev);
        next.delete(id);
        return next;
      });
    }
  };

  return (
    <div className="fixed inset-0 bg-background/80 backdrop-blur-sm z-50 flex items-center justify-center p-4">
      <div className="bg-surface border border-border rounded-xl shadow-xl max-w-md w-full overflow-hidden flex flex-col max-h-[85vh]">

        {/* Header */}
        <div className="px-6 py-4 border-b border-border flex items-center justify-between sticky top-0 bg-surface">
          <div className="flex items-center gap-3">
            <div className="p-2 bg-gruvbox-blue/10 rounded-lg text-gruvbox-blue">
              <EnvelopeIcon className="w-5 h-5" />
            </div>
            <div>
              <h2 className="text-lg font-medium text-primary">Pending Invitations</h2>
              <p className="text-sm text-secondary">Secure end-to-end encrypted invites</p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="p-2 text-tertiary hover:text-primary rounded-lg hover:bg-surface-hover transition-colors"
          >
            <XMarkIcon className="w-5 h-5" />
          </button>
        </div>

        {/* List Body */}
        <div className="p-6 overflow-y-auto flex-1">
          {error && (
            <div className="mb-4 p-3 bg-gruvbox-red/10 border border-gruvbox-red/20 rounded-lg text-sm text-gruvbox-red">
              {error}
            </div>
          )}

          {pendingInvites.length === 0 ? (
            <div className="text-center py-8">
              <div className="inline-flex justify-center items-center w-12 h-12 rounded-full bg-surface-hover text-tertiary mb-3">
                <EnvelopeIcon className="w-6 h-6" />
              </div>
              <h3 className="text-primary font-medium">Your inbox is empty</h3>
              <p className="text-secondary text-sm mt-1">No pending invitations found.</p>
            </div>
          ) : (
            <div className="space-y-4">
              {pendingInvites.map((invite) => {
                const id = inviteId(invite);
                const isLoading = loadingIds.has(id);
                return (
                  <div key={id} className="p-4 rounded-xl border border-border bg-surface-hover transition-all">
                    <div className="flex items-start justify-between gap-4 mb-4">
                      <div className="flex items-center gap-3">
                        <div className="p-2 bg-background rounded-lg border border-border">
                          <BuildingOfficeIcon className="w-5 h-5 text-secondary" />
                        </div>
                        <div>
                          <h4 className="text-primary font-medium">{invite.org_name || 'Unknown Organization'}</h4>
                          <p className="text-xs text-tertiary font-mono truncate max-w-[200px]">
                            {(invite.org_public_key || '').slice(0, 12)}...
                          </p>
                        </div>
                      </div>
                    </div>

                    <div className="text-sm text-secondary space-y-1 mb-5 bg-background p-3 rounded-lg border border-border font-mono">
                      <div><span className="text-tertiary">Role:</span> {invite.invited_role || 'member'}</div>
                      <div><span className="text-tertiary">From:</span> {invite.invited_by?.slice(0,8) || 'admin'}...</div>
                    </div>

                    <div className="flex gap-2 w-full">
                      <button
                        onClick={() => handleAccept(invite)}
                        disabled={isLoading}
                        className="flex-1 btn-primary flex justify-center items-center gap-2 font-medium"
                      >
                        {isLoading ? <span className="animate-spin text-lg">↻</span> : <CheckIcon className="w-4 h-4" />}
                        Accept
                      </button>
                      <button
                        onClick={() => handleDecline(invite)}
                        disabled={isLoading}
                        className="flex-1 btn-secondary flex justify-center items-center gap-2"
                      >
                        <XMarkIcon className="w-4 h-4" />
                        Decline
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
