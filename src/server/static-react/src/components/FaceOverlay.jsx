import { useState, useEffect } from 'react';
import { discoveryClient } from '../api/clients/discoveryClient';
import { listContacts } from '../api/clients/trustClient';

export default function FaceOverlay({ schemaName, recordKey }) {
  const [faces, setFaces] = useState([]);
  const [loading, setLoading] = useState(true);
  const [searchResults, setSearchResults] = useState(null);
  const [searchingFace, setSearchingFace] = useState(null);
  const [knownPseudonyms, setKnownPseudonyms] = useState(() => new Set());

  useEffect(() => {
    let cancelled = false;
    async function loadFaces() {
      try {
        const resp = await discoveryClient.listFaces(schemaName, recordKey);
        if (!cancelled && resp.success && resp.data?.faces) {
          setFaces(resp.data.faces);
        }
      } catch {
        // silently ignore — face detection may not be enabled
      }
      if (!cancelled) setLoading(false);
    }
    loadFaces();
    return () => { cancelled = true; };
  }, [schemaName, recordKey]);

  useEffect(() => {
    let cancelled = false;
    async function loadContacts() {
      try {
        const resp = await listContacts();
        if (cancelled || !resp.success) return;
        const contacts = resp.data?.contacts || [];
        const set = new Set();
        for (const c of contacts) {
          if (c.pseudonym) set.add(c.pseudonym);
          if (c.messaging_pseudonym) set.add(c.messaging_pseudonym);
        }
        setKnownPseudonyms(set);
      } catch {
        // best-effort
      }
    }
    loadContacts();
    return () => { cancelled = true; };
  }, []);

  async function handleConnect(pseudonym) {
    try {
      const resp = await discoveryClient.connect(pseudonym, 'Face match from discovery', 'acquaintance');
      if (resp.success) {
        window.alert('Connection request sent');
      } else {
        window.alert(`Failed: ${resp.error || 'unknown'}`);
      }
    } catch (e) {
      window.alert(`Error: ${e?.message || e}`);
    }
  }

  async function searchFace(faceIndex) {
    setSearchingFace(faceIndex);
    setSearchResults(null);
    try {
      const resp = await discoveryClient.faceSearch(schemaName, recordKey, faceIndex);
      if (resp.success) {
        setSearchResults(resp.data?.results || []);
      }
    } catch {
      setSearchResults([]);
    }
    setSearchingFace(null);
  }

  if (loading || faces.length === 0) return null;

  return (
    <div className="mt-2">
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-xs text-secondary">{faces.length} face(s) detected:</span>
        {faces.map(f => (
          <button
            key={f.face_index}
            type="button"
            className="px-2 py-0.5 text-xs rounded-full border border-border hover:border-gruvbox-blue hover:bg-gruvbox-blue/10 transition-colors bg-transparent text-primary cursor-pointer"
            onClick={() => searchFace(f.face_index)}
            disabled={searchingFace !== null}
          >
            {searchingFace === f.face_index ? 'Searching...' : `Face ${f.face_index}`}
          </button>
        ))}
      </div>
      {searchResults !== null && (
        <div className="mt-2 p-2 bg-surface-secondary rounded border border-border">
          {searchResults.length === 0 ? (
            <p className="text-xs text-secondary">No matches found on the network.</p>
          ) : (
            <div className="space-y-1">
              <p className="text-xs text-secondary font-medium">{searchResults.length} match(es):</p>
              {searchResults.map((r, i) => (
                <div key={i} className="flex items-center justify-between text-xs p-1.5 rounded bg-surface">
                  <div className="flex items-center gap-2">
                    <span className="text-primary">{r.category || 'face'}</span>
                    <span className="text-gruvbox-green">{(r.similarity * 100).toFixed(1)}%</span>
                  </div>
                  <div className="flex items-center gap-2">
                    <code className="text-tertiary text-[10px]">{String(r.pseudonym).slice(0, 8)}...</code>
                    {knownPseudonyms.has(r.pseudonym) ? (
                      <span className="text-[10px] px-2 py-0.5 rounded-full bg-gruvbox-green/10 text-gruvbox-green border border-gruvbox-green/30">
                        ✓ Already connected
                      </span>
                    ) : (
                      <button
                        type="button"
                        onClick={() => handleConnect(r.pseudonym)}
                        className="text-[10px] px-2 py-0.5 rounded border border-gruvbox-blue/40 text-gruvbox-blue hover:bg-gruvbox-blue/10 transition-colors bg-transparent cursor-pointer"
                      >
                        Connect
                      </button>
                    )}
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
