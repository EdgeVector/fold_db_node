// Key Management Tab wrapper component

import { useState, useEffect, useRef } from 'react';
import { useAppSelector } from '../../store/hooks';
import { ShieldCheckIcon, ClipboardIcon, CheckIcon } from '@heroicons/react/24/outline';

function KeyManagementTab({ onResult: _onResult }) {
    // Redux state
    const authState = useAppSelector(state => state.auth);
    const { isAuthenticated, systemPublicKey, systemKeyId, isLoading } = authState;

    const [copiedField, setCopiedField] = useState(null);
    const copiedTimeoutRef = useRef(null);

    useEffect(() => {
      return () => { if (copiedTimeoutRef.current) clearTimeout(copiedTimeoutRef.current) };
    }, []);

    const copyToClipboard = async (text, field) => {
        try {
            await navigator.clipboard.writeText(text);
            setCopiedField(field);
            if (copiedTimeoutRef.current) clearTimeout(copiedTimeoutRef.current);
            copiedTimeoutRef.current = setTimeout(() => setCopiedField(null), 2000);
        } catch (err) {
            console.error('Failed to copy:', err);
        }
    };

    return (
        <div className="p-6 space-y-4">
            {/* Current System Public Key Display */}
            <div className="card card-info p-4">
                <div className="flex items-start">
                    <ShieldCheckIcon className="h-5 w-5 text-gruvbox-blue mr-2 flex-shrink-0 mt-0.5" />
                    <div className="text-sm text-primary flex-1">
                        <p className="font-medium text-gruvbox-blue">Current System Public Key</p>
                        {isLoading ? (
                            <p className="text-secondary">Loading...</p>
                        ) : systemPublicKey ? (
                            <div className="mt-2">
                                <div className="flex">
                                    <input
                                        type="text"
                                        value={systemPublicKey && systemPublicKey !== 'null' ? systemPublicKey : ''}
                                        readOnly
                                        className="flex-1 px-2 py-1 text-xs font-mono bg-surface border border-border text-primary focus:outline-none focus:border-primary"
                                    />
                                    <button
                                        onClick={() => copyToClipboard(systemPublicKey, 'system')}
                                        className="px-2 py-1 border border-border border-l-0 bg-surface-secondary focus:outline-none cursor-pointer hover:bg-surface transition-colors"
                                    >
                                        {copiedField === 'system' ? (
                                            <CheckIcon className="h-3 w-3 text-gruvbox-green" />
                                        ) : (
                                            <ClipboardIcon className="h-3 w-3 text-gruvbox-blue" />
                                        )}
                                    </button>
                                </div>
                                {systemKeyId && (
                                    <p className="text-xs text-secondary mt-1">Key ID: {systemKeyId}</p>
                                )}
                                {isAuthenticated && (
                                    <p className="text-xs text-gruvbox-green mt-1">Authenticated</p>
                                )}
                            </div>
                        ) : (
                            <p className="text-secondary mt-1">No system public key available.</p>
                        )}
                    </div>
                </div>
            </div>
        </div>
    );
}

export default KeyManagementTab;
