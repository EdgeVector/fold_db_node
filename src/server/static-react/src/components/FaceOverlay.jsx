import { useState, useEffect } from 'react';
import { discoveryClient } from '../api/clients/discoveryClient';

export default function FaceOverlay({ schemaName, recordKey }) {
  const [faces, setFaces] = useState([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState(null);
  const [searchResults, setSearchResults] = useState(null);
  const [searchingFace, setSearchingFace] = useState(null);

  useEffect(() => {
    let cancelled = false;
    async function loadFaces() {
      try {
        const resp = await discoveryClient.listFaces(schemaName, recordKey);
        if (cancelled) return;
        if (resp.success && resp.data?.faces) {
          setFaces(resp.data.faces);
        } else if (!resp.success) {
          // Surface the error explicitly. This may be expected in builds
          // where the face-detection cargo feature is disabled — we cannot
          // distinguish "feature off" from "runtime crash" here, so we
          // show whatever the backend returned and warn (not error) in
          // the console.
          const msg = resp.error || 'Face detection unavailable';
          console.warn(`FaceOverlay: listFaces failed: ${msg}`);
          setLoadError(msg);
        }
      } catch (e) {
        if (cancelled) return;
        const msg = e?.message || String(e);
        console.warn(`FaceOverlay: listFaces threw: ${msg}`);
        setLoadError(msg);
      } finally {
        if (!cancelled) setLoading(false);
      }
    }
    loadFaces();
    return () => { cancelled = true; };
  }, [schemaName, recordKey]);

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
      } else {
        setSearchResults([]);
      }
    } catch {
      setSearchResults([]);
    }
    setSearchingFace(null);
  }

  if (loading) return null;

  if (loadError) {
    return (
      <div className="mt-2">
        <span className="text-xs text-tertiary">
          Face detection unavailable (feature not enabled in this build).
        </span>
      </div>
    );
  }

  if (faces.length === 0) return null;

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
                    {/*
                      NOTE: "Already connected" badge removed. The pseudonyms
                      returned from face search are derived from face embedding
                      bytes (see src/discovery/publisher.rs `content_hash_bytes`)
                      and live in a different namespace from text-contact
                      pseudonyms — comparing them never matched in practice.
                      Follow-up: track face-pseudonym <-> contact mapping at
                      connect-accept time so this badge can be restored.
                    */}
                    <button
                      type="button"
                      onClick={() => handleConnect(r.pseudonym)}
                      className="text-[10px] px-2 py-0.5 rounded border border-gruvbox-blue/40 text-gruvbox-blue hover:bg-gruvbox-blue/10 transition-colors bg-transparent cursor-pointer"
                    >
                      Connect
                    </button>
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
