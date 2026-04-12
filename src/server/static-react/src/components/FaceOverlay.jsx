import { useState, useEffect } from 'react';
import { discoveryClient } from '../api/clients/discoveryClient';

export default function FaceOverlay({ schemaName, recordKey }) {
  const [faces, setFaces] = useState([]);
  const [loading, setLoading] = useState(true);
  const [searchResults, setSearchResults] = useState(null);
  const [searchingFace, setSearchingFace] = useState(null);

  useEffect(() => {
    let cancelled = false;
    async function loadFaces() {
      try {
        const resp = await discoveryClient.listFaces(schemaName, recordKey);
        if (!cancelled && resp.success && resp.data?.faces) {
          setFaces(resp.data.faces);
        }
      } catch (e) {
        // silently ignore — face detection may not be enabled
      }
      if (!cancelled) setLoading(false);
    }
    loadFaces();
    return () => { cancelled = true; };
  }, [schemaName, recordKey]);

  async function searchFace(faceIndex) {
    setSearchingFace(faceIndex);
    setSearchResults(null);
    try {
      const resp = await discoveryClient.faceSearch(schemaName, recordKey, faceIndex);
      if (resp.success) {
        setSearchResults(resp.data?.results || []);
      }
    } catch (e) {
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
                  <code className="text-tertiary text-[10px]">{String(r.pseudonym).slice(0, 8)}...</code>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
